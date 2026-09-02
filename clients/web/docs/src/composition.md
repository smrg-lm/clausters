# Composition: the arrangement and the multitrack editor

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is not a flat list of events,
it is an element inside an element — a phrase inside a section inside a piece, a
take placed against a melody, a generator that has not been evaluated yet.

The `form` namespace is that layer — the **arrangement model**. It is the same
layer the Python client has, in this language: the two write the same document
and flatten to the same timeline, and a parity suite holds them to it.

`gui.FormEditor` puts it on screen as a multitrack view you can edit. The point of
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
wraps.

**The two are not in the same unit, and each takes its own from what it answers
to.** An onset is in **beats**, always: placing something is a musical decision,
and it takes the unit of what contains it. A duration is in the unit of the
element's own data — **seconds** for a `Vector`, a `Segments` or a curve, because
a recording's length is `frames / sampleRate` and no tempo change makes it
shorter; **beats** for a `Clang`, a `Sequence` or a `Track`, because a note *is*
musical and a tempo change is supposed to shorten it. `Element.durationUnit` says
which, derived from what the element holds rather than stored beside it. The
conversion happens where the tree is flattened for playback (`render`, which
reads the clock's tempo) and never in the tree, since a timeline is ordered by
one number and cannot hold two bases. The arrangement is a thin adornment over what the client already has, not
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

// a def that plays a buffer, sounding two seconds of it
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

## Two editors, and which one is which

`gui.Editor` edits **one structure** — a buffer's samples, a break-point curve, a
timeline of events — and it knows nothing about the arrangement.
`gui.FormEditor` is that class plus what only a tree has: a held document,
several views of one composition, the lanes and clips, and a transport. The names
say which is which, and the general one has the plain name because editing a
curve is the plain case.

An editor orchestrates rather than performs, and it is four collaborators
(`gui/editing/`):

| | what it is | what it deliberately is not |
|---|---|---|
| `View` | the picture of one structure, and the registry from widget id to what it shows | not the vocabulary: one structure is drawn several ways |
| `Domain` | gesture → payload, payload → the client object, the label, the coalesce key | not **how an edit inverts** — that is the shared crate's, so it is not written once per language — and it does not draw |
| `Echo` | the acknowledgement: the stamp, the version, the corrections, the reason | not anything about what was edited |
| `Editing` | the editing context: the history, and the views to tell | **not the editor's** — it is asked for, never built, which is what makes two windows walk one undo order |

The rule that fixes all four: an editor owns **neither the data nor the
history**. `View` here is not `guidef`'s `View`, which is a tree you can open.

## `edit(x)`: one verb over the three structures

`gui.edit` opens whichever editor the structure asks for, and it dispatches on
**what the structure is** — that being the question a caller has already answered
by holding one:

| `edit(x)` where x is | opens | over | its vocabulary |
|---|---|---|---|
| a `Buffer` | `SamplesEditor` | a `waveform` | `samples` |
| an `Automation` | `PointsEditor` | a `bpf` | `points` |
| a `Timeline` | `NotesEditor` | a `pianoroll` | `events` |

```ts
const editor = gui.edit(curve, { sampleRate: 48_000 });
await editor.open(undefined, { stage: element });

curve.toPoints();      // the edited curve, out of the object you already held
```

Nothing is handed back: the object passed in *is* the edited one. A composition
is not one of the three — an arrangement is `FormEditor`'s, which knows a tree
from a leaf and holds a document.

**Two calls over one structure give two windows and one stack.** The editing
context belongs to the data, so an undo in either window steps the one order both
of them made. And a window composing several structures passes one context
(`edit(x, { context })`), which is what makes it undo across a curve and a roll
in the order the edits happened.

**How an edit inverts is the shared crate's.** For a curve and a timeline the
state goes in with the payload and comes back as what the structure now is *plus*
what puts it back — one call, because the inverse has to be read before the edit
lands. A span of samples is the exception, and a real one rather than an
omission: the frames are in a server buffer, so the crate holds no state to
invert. What it shares there is the payload's shape and its coalesce key, and the
inverse rides on the wire — a stroke's event carries the run it wrote *and* the
run it replaced. The page's buffer calls are asynchronous, so a stroke's write is
**queued in order** rather than awaited; the Python client writes synchronously,
and that is the only difference between the two.

## The multitrack editor

`FormEditor` draws that tree as the multitrack view and applies its edits back onto
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
const editor = new gui.FormEditor(song, {
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
beats and measures each one's length in the unit of its own data; the view places
clips in timeline samples, because a clip's body is audio data. A length in
seconds crosses on `unitsPerSecond` (the rate itself) and an onset crosses on the
piece's **time map**, so a take is drawn exactly as wide as it sounds whatever the
tempo is and only its placement follows the grid.

The map is what makes the placement side right when the tempo changes. A beat is
a logical coordinate: what second it falls on depends on the whole tempo history
before it, not on the tempo in force now, so the same four beats are a different
stretch of the axis at the start of the piece and after an accelerando. That is
why the editor holds a `TempoMap` rather than a number, why `beatsToUnits` takes
a position, and why `lengthToUnits` takes the onset a length starts at. Under a
single tempo it is the plain ratio `sampleRate / tempo`, which is what
`unitsPerBeat` still names.

Hand the editor the clock's map when the piece changes tempo, so the line and the
sound are one function rather than two readings of it:

```js
clock.setTempo(2.0);                       // or clock.setTempo(2.0, { over: 8 })
const editor = new FormEditor(song, { sampleRate, tempoMap: clock.map });
```

`editor.render(server, clock)` adopts the clock's map anyway and redraws if it
moved, so the two cannot silently disagree — but passing it up front means the
first draw is already right.

**Where an edit is decided is not here either.** A drag leaves the host as an
intent — where the hand put it, absolute — and the shared crate applies it: the
`quant` grid snaps it *there*, and what comes back is the value that actually
holds, which the editor projects onto the arrangement and answers the host with.
So a clip lands on the grid although nothing in this client snapped anything, and
`undo()`/`redo()` walk the crate's pile rather than a history a view kept for
itself.

That pile belongs to the **arrangement**, so two windows over one piece find the
same one:

```ts
const multitrack = new FormEditor(piece, { sampleRate });
const roll = new FormEditor(piece, { sampleRate });   // a second window, same piece

await multitrack.open(host);
await roll.open(host);             // `open` listens, so nothing is pumped here

roll.canUndo;                      // true after a drag in the other window
roll.undo();                       // and the clip springs back in both
```

An edit in one window **reaches** the others, which nothing else would do: an
acknowledgement goes to the window whose gesture it answered. It arrives as
props — the placement, the length, the notes — and only a structural edit (a
split, a cut, an undo of one) redraws them whole, for the same reason a redefine
is not what answers a drag. (This is where the
two clients differ in shape and not in surface: `open` subscribes here, while a
script feeds one poll loop to every editor it opened.)

A history an editor kept would see only the gestures *that* editor made, so a
script editing the arrangement or a second view would leave it describing a
composition that has moved on — and undoing then writes a state nobody was ever
in. It holds for a window over a *part* of the piece too: a dedicated roll of
one track edits through the composition's history rather than opening a second
one over the same notes. What each window keeps for itself is what a window can
see — its selection, its zoom, which layer the hand is on (`editor.editLayerOf`)
— and none of that is ever an entry in a history. What a view keeps is asked for
by the **placement**, the way every other route is: a widget id is the picture's
name for a widget and is minted afresh every time the window is redrawn, so
nothing that has to outlive a redraw is keyed by one.

A trim, a split and a join are the same round trip in the same one edit each: a
placement is a **window onto** an element, so shortening a clip over its own
notes plays fewer of them and keeps them all — lengthen it again and they come
back.

**Splitting and joining.** With the pointer over a clip, `e` cuts it in two at
the time cursor (at the pointer when no cursor is inside it) and `j` joins it
with the clips that touch it on its lane. Neither is a menu item or an
affordance: they are the clip's own verbs, addressed by the pointer like every
other verb over a view. A split gives each half a window over the same samples,
which is why a join can put back exactly what the cut separated.

A **structural** edit redraws itself: a split, a join or a cut changes which
clips exist, and a widget that was not there cannot travel as a property, so the
editor redefines the window — for the edit and for an undo of it. A placement, a
length or a curve travels with the acknowledgement instead.

An intent states the **whole** value, and **absence is a value**: a `place`
carrying no `dur` is a placement with *no length*, and the element's own is what
plays. That is what an undo of the first resize of a clip hands back — before it
there was no length to restore — and the same holds one level down, where a
member with no configuration is a leaf configured as it was made.

A note edited in a roll is **updated, not rebuilt**: the event keeps its
instrument and everything else the roll cannot show, and the length a drag on
its edge sets is the note's `sustain` — which is what the bar draws — so its
`dur` and `legato` stay as they were written.

**Cut, copy and paste are one mechanism.** A block of notes copied out of a roll
(`Ctrl+C`) is written onto the roll a paste addresses as an ordinary edit of its
notes — the same call a drag on a note goes through — so it is one entry on the
pile, and one undo takes the whole block back. The position a paste names is on
the *timeline's* axis while a roll's notes are in its clip's own time, so a clip
placed at beat 2 holds its own note 0 there: the editor converts, and the block
keeps the spread it was copied with. A cut whose selection covers a clip removes
that placement, undoably; a cut running across one, and a paste of a block of
*samples*, are refused with the reason — audio with neither a source nor a
source's owner is not something an editor of placements may invent.

Two dedicated views open on one element **beside** the multitrack, not instead
of it: `openPianoroll(host, element)` for an editable note grid, and
`openSignal(host, element)` for the editor-grade waveform of a rendered element
(its `layers` — `["peak", "rms"]` — is a live prop, not a pile of widgets).

Each **composes an editor of its own** — the `NotesEditor` `edit(timeline)`
opens, the `SamplesEditor` `edit(buffer)` opens — joined to *this* composition's
editing context, and reachable as `editor.composed`. So the windows step **one**
history: a note moved in a roll reaches the clip drawing it as props, without
either window being redefined, and a stroke drawn on a take is this piece's edit
in the crate's `samples` vocabulary. The multitrack cannot read a samples leg at
all; it hands that leg to the editor that can, which is what one editing context
over several structures is for — so a stroke and a clip's move undo in the order
your hand made them, from whichever window has focus.

A generator has no samples until it is rendered, so `openSignal` refuses one and
says what to do; `openPianoroll` bounces what it produced onto a timeline of its
own and opens the roll **read-only**, telling the widget so rather than refusing
each drag after the hand has made it.

The examples are `examples/editors/composer.html` and
`examples/editors/composed.html`.

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

The table is not something to keep by hand. `sourcesOf` builds it from the
arrangement being saved — each take's buffer asked where it is — which is what
keeps it covering the piece as the piece changes:

```ts
const session = form.toSession(song, {
    sources: Object.fromEntries(form.sourcesOf(song, { folder: "pieces/one" })),
});
```

A buffer read from a file knows its `path` and is written as that file; one
allocated in this run is written **volatile** — it existed only while the page
did, and a session that promised otherwise would reopen with silence where it
promised samples. A path inside the session's own folder is written relative, so
the pair of files moves together; one outside it stays absolute, because a
session never claims to own the user's file.

## Reopening: structures, not a description

`fromSession` rebuilds the tree, and by itself that is half a verb: every take
comes back as a bare source number and nothing loads it. A **resolver over the
session's own table** is the other half:

```ts
const resolve = await form.sessionResolver(saved, { folder, defs });
const { element, sources } = form.fromSession(saved, { resolve });
```

Each file the table names is read onto the server **once per source** — two clips
over one take are two windows onto one buffer, and reading it twice gives them
two buffers that drift apart on the first edit. A *volatile* source comes back
frozen rather than as a lie. A file that has moved comes back frozen too, and the
rest of the piece opens: half a session is worth opening. And a generator whose
reference `defs` does not have keeps what it last **rendered** as its floor,
which is the same thing a host with no language attached shows.

Reading a file is asynchronous here and not in the Python client (a page's
`Buffer.read` goes to the worker that owns the filesystem), so the takes are read
while the resolver is built and the resolver itself is the same synchronous
function on both sides — the `await` is the language's, not a different call.

## Mixing is the composition's

Every element carries `mute`, `solo` and `level`, and all three are inherited
down the tree: muting an aggregate silences its members, one soloed element
anywhere silences every branch that is not on a soloed path, and a level
multiplies into the `amp` of the events under it.

```ts
bassLane.mute = true;
leadLane.level = 0.5;
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

## Where a recording lands

A `Buffer` holds a take and a `RecordingStream` follows one as it is written, and
neither puts one in a piece. `take` does:

```ts
song.add(form.take(recorded, null, null, { instrument: "player" }), 8.0);
```

It is a `Vector` whose length is the samples' own — frames over the rate they
were recorded at — which is the one line every script used to write by hand.
Without an `instrument` it is structure: it draws and it extends the piece, and
it emits no event, which is the `Vector` rule rather than a special case.
