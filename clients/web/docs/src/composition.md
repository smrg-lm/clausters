# Composition: the arrangement and the multitrack editor

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is not a flat list of events,
it is an element inside an element — a phrase inside a section inside a piece, a
take placed against a melody, a generator that has not been evaluated yet.

The `form` namespace is that layer — the **arrangement model**. It is the same
layer the Python client has, in this language: the two write the same document
and flatten to the same timeline, and a parity suite holds them to it.

`gui.Editor` puts it on screen as a multitrack view you can edit. The point of
the pair is that the graphic is not a picture of the music: dragging a clip moves
the *element*, and the sound follows.

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
can be evaluated, in order, and that is all. An element carries two optional
temporal properties — an `onset` (where it starts, in beats, relative to its
context) and a `duration` — and delegates the actual playing to the object it
wraps. The arrangement is a thin adornment over what the client already has, not
a second implementation of it.

Which of the two properties are present gives an element its temporal
*character*: both is a **segment**, an onset alone is **punctual**, a duration
alone is **relative** (it has a length but no place yet), neither is **abstract**
— pure context, which only a parent gives concrete time.

There are five kinds, and they map one to one onto objects you already use
(`Segments` is not a sixth: it is the `Vector` primitive — a list at constant
time — assembled from more than one window):

| Element     | What it is                                       | Wraps                              |
| ----------- | ------------------------------------------------ | ---------------------------------- |
| `Clang`     | parameters grouped into one action               | `Event`                            |
| `Sequence`  | strict order, no concrete time — only sequence   | an array, or a `Pattern`           |
| `Vector`    | a list at constant time (samples)                | `Buffer`                           |
| `Segments`  | several windows onto samples, read as one        | `[buffer, start, duration]` triples |
| `Track`     | mixed placement of elements — a DAW track        | `Timeline`                         |
| `Generator` | a *process*: server DSP, or a sequence generator | a def, or a `Pbind`/`Routine`      |

A `Vector` is *data*, so it has no sound of its own: it sounds through the
**instrument** named to play it — a def whose `buf` control takes the buffer
number. That is the whole rule for an audio clip. A `Segments` is the same rule
over several of them: it is what assembling samples out of pieces looks like when
nothing is copied.

```ts
import { form } from "clausters";

// a def that plays a buffer, sounding two beats of it
const take = new form.Vector(buf, null, 2.0, { instrument: "take" });
```

The two positional arguments after what an element wraps are always its `onset`
and its `duration`; everything else is named.

## Grouping: the one new structure

An `Aggregate` places elements by an offset, recursively — and that recursion is
the whole idea. It comes in two kinds. A **concrete** aggregate is a relation *in
time* between its members (a section holding clips, a melody holding notes). A
**logical** aggregate is a relation of *processing*: the members are wired to each
other through buses, which is exactly what a `GraphDef` expresses, so
`aggregate.toGraphdef()` translates one into it.

```ts
const song = new form.Aggregate([
    [0.0, new form.Aggregate([[0.0, take], [4.0, take]], "concrete", { name: "drums" })],
    [2.0, new form.Aggregate([[0.0, melody]], "concrete", { name: "lead" })],
], "concrete", { name: "song" });
```

The `take` above is placed **twice**, which is the ordinary thing to write and
means what it says: two clips, one take. A placement is a **window onto samples** —
editing the samples through either window edits the one take, and moving one clip
moves that clip. What can be placed twice is samples the element only *names*: a
`Vector` over a server buffer, a `Generator` over a pattern or a def. An element
that carries its samples *inside* it — a `Clang`, a `Track`, an `Aggregate` — is
refused, because two placements of one of those would be two copies that diverge
the moment you edit one.

From how its members sit in time, an aggregate *derives* its temporal
**relation**: `successive` when they tile contiguously, `simultaneous` when they
start and end together, `mixed` otherwise. You do not set it; it is read from the
placements.

A placement may also carry a length of its own, and that length is what you hear
of what it holds: events past its end are dropped and a single-event element
sounds for exactly that long — the DAW rule, and what resizing a clip changes.

## Rendering: the change of state

Rendering a composition **flattens** it — a tree-walk accumulating the nested
offsets into absolute beats — into a flat `Timeline`, which a `Playhead` then
plays. A generator contained in it is *bounced* in the same pass: that evaluation,
the change from a process into a generated element, is the *change of state*.

```ts
const playhead = song.render(server, clock);   // live, through a playhead
```

There is no second rendering path: what differs between destinations is the
destination, not the flattening.

A **logical** aggregate takes the other path entirely — its `GraphDef` is sent and
instanced on the server — so `render` there answers with a promise of the
instance group rather than a playhead. Sending a def is a round trip, and this
client awaits one rather than blocking the page's single thread.

An element is *rendered*, never played: `play` is for what already sounds
directly, and a flat `Timeline`, being already generated, is playable.

## The multitrack editor

`Editor` draws that tree as the multitrack view and applies its edits back onto
the tree. The mapping is one rule, not a heuristic per case:

- the root aggregate's members are the **lanes**; a lane's members are its
  **clips**;
- a `Vector` clip names its **server buffer** and spans its frames — the host
  fetches and decimates it, so a real take never rides the wire as JSON;
- an element of events draws a **piano roll**, and a contained generator is
  bounced in the same pass, so a pattern lane shows the notes it is about to
  play;
- an `Automation` draws its **curve** as the clip body, editable in place;
- a nested aggregate draws as the labeled rectangle that summarizes it, until
  `expand` resolves it into lanes of its own — the arrangement's *base level*;
- a **logical** aggregate draws as a directed `patch` instead of a lane: a box
  per member, its ports typed from the def it wraps, and a cord drawn there
  rewrites the members onto a shared bus.

The same patcher draws a def **on its own**, as a way to *look at its
structure*. `someDef.plotDef()` opens one window per call showing the def as a
directed patch — distinct from `plot(someDef)`, which shows the def's *sound*
(its rendered waveform). It reads at **two levels**: a `GraphDef` draws as its
member nodes wired by buses (the same picture the logical aggregate shows); a
`SynthDef` or `FaustDef` draws one level deeper, as its **internal graph** —
every UGen (or Faust signal op) a box, every input a cord, the def's controls the
source boxes and its literals small value boxes. A cord is coloured by rate —
contrasting primaries at one width, audio red, control blue, and level 2's third,
**init** (`ir`, yellow and dashed), a scalar read once at init time. The host lays
the boxes out on its own. The Def view is **read-only** — the faithful picture of
what the def is (`DefPatch.fromSynthdef(sdef).toSynthdef(name)` reproduces the
original spec); it needs no audio server.

```ts
const editor = new gui.Editor(song, {
    sampleRate: engine.context.sampleRate,
    tempo: clock.tempo,
    quant: 0.5,              // the musical drag grid
    follow: true,            // what is sounding follows the edit
});
const win = editor.open(host);       // draw, open, and listen
await editor.render(server, clock);  // play the composition as it now stands
```

`open` **subscribes** — a page has an event loop, so every `/gui_event` reaches
the editor as it arrives, where a script in the Python client drains a queue
itself. `detach()` stops it.

**Beats meet samples here, and only here.** The arrangement places elements in
beats; the view places clips in timeline samples, because a clip's body is audio
data. One beat is `sampleRate / tempo` units, so a take placed at its own length
sits 1:1 on the axis.

**Where an edit is decided is not here either.** A drag leaves the host as an
intent — where the hand put it, absolute — and the shared crate applies it: the
`quant` grid snaps it *there*, and what comes back is the value that actually
holds, which the editor projects onto the arrangement and answers the host with.
So a clip lands on the grid although nothing in this client snapped anything, and
`undo()`/`redo()` walk the crate's log rather than a history a view kept for
itself.

A trim, a split and a join are the same round trip in the same one edit each: a
placement is a **window onto** an element, so shortening a clip over its own
notes plays fewer of them and keeps them all — lengthen it again and they come
back.

A **structural** edit redraws itself: a split, a join or a cut changes which
clips exist, and a widget that was not there cannot travel as a property, so the
editor redefines the window — for the edit and for an undo of it. A placement, a
length or a curve travels with the acknowledgement instead.

An intent states the **whole** value, and **absence is a value**: a `place`
carrying no `dur` is a placement with *no length*, and the element's own is what
plays. That is what an undo of the first resize of a clip hands back — before it
there was no length to restore — and the same holds one level down, where a
member with no configuration is a leaf configured as it was made.

Two dedicated views open on one element instead of the multitrack:
`openPianoroll(host, element)` for an editable note grid, and
`openSignal(host, element)` for the editor-grade waveform of a rendered element
(its `layers` — `["peak", "rms"]` — is a live prop, not a pile of widgets).

The example is `examples/composer.html`.

## The document: what the composition *is*

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the Python client, and a GUI host running standalone with
no language attached at all.

`toDocument` writes the arrangement as the document, and `fromDocument` reads one
back:

```ts
const doc = form.toDocument(song);        // { version: 1, root: {...} }
const songAgain = form.fromDocument(doc);
```

The conversion is lossless for concrete samples — clangs, placements, aggregates,
vectors by reference — and carries a **generator by reference**, the way a project
file references a plugin rather than serializing it. A generator *is code*, in the
language that wrote it, so no format owns one; what the document guarantees is
that it does not lose it. Reading one back, `fromDocument(doc, { resolve })` hands
each named leaf to your resolver and takes back whatever it has; with no resolver
the reference itself stays in place, and that leaf is **frozen** — it draws, it
holds its place, and it makes no sound. That is the floor, not a failure: it is
what a composition means where the language that wrote it is not running.

Node ids are stamped onto the elements, so converting the same tree twice gives
the same ids and an edit made against one conversion still names the right node in
the next.

Because a document is one format for several languages, the two event keys this
language spells its own way (`addAction`, `hasGate`) are written the way the file
and the wire say them (`add_action`, `has_gate`) and read back the same way. Every
other key is a def's control name, which is one string in every language.

## Saving: the document plus where its sources are

A document says what plays when and deliberately not where a source lives — in a
running system a source is a server buffer, a mapped file or a rendered result,
and the tree has no business knowing which. A **session** is the document plus
that missing half:

```ts
const session = form.toSession(song, {
    sources: {
        7: { location: { at: "file", path: "takes/vocal.wav" },
             lifetime: "external", generation: 0 },
    },
    provenance: { page: "song.html" },
});
const { element, sources } = form.fromSession(session);
```

A source's **lifetime** is what makes saving honest: `external` is the user's own
file, which is never written; `session` is persisted beside the document;
`temporary` is a destructive edit's working copy. A session whose table does not
cover its own document is refused as it is written, rather than reopening with a
take that draws nothing and nothing saying why.

`provenance` is a reference to whatever produced something, carried and never
interpreted. It is what makes re-generating possible without the format knowing
how, which is the same rule the opaque generator follows one level down.
