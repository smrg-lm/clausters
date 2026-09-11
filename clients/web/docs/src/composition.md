# Composition: the arrangement, and the document under it

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is not a flat list of events,
it is an element inside an element — a phrase inside a section inside a piece, a
take placed against a melody, a generator that has not been evaluated yet.

The `form` namespace is one such layer — a small, self-contained set of
client-side data structures for placing elements in time. It is the same layer the
Python client has, in this language: the two write the same document and flatten
to the same timeline, and a parity suite holds them to it.

It is **relegated**: it has no view, it takes no new work, and the arrangement an
application is built on lives in the **document** described in the second half of
this chapter.

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

A `Sequence` of elements is laid out **one after another**, and what it advances
by is each item's own `duration` — its stated length in its own unit. An item
that states none is as long as *what it lays down*, which is what a `Sequence`
of `Sequence`s relies on: a bar says nothing about its length, and the four
notes in it say everything. (Mute and solo do not enter: they say what is
heard, never where anything is, so silencing one member leaves the ones after
it where they were.)

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

## What an editor is

`gui.Editor` edits **one structure** — a buffer's samples, a break-point curve, a
timeline of events — and it knows nothing about any arrangement. That is the whole
of it, and it is deliberately the plain case: editing a curve is what an editor is
for.

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

## `edit(x)`: one verb over the four structures

`gui.edit` opens whichever editor the structure asks for, and it dispatches on
**what the structure is** — that being the question a caller has already answered
by holding one:

| `edit(x)` where x is | opens | over | its vocabulary |
|---|---|---|---|
| a `Buffer` | `SamplesEditor` | a `waveform` | `samples` |
| an `Automation` | `PointsEditor` | a `bpf` | `points` |
| a `Timeline` | `NotesEditor` | a `pianoroll` | `events` |
| a `Multitrack` | `MultitrackEditor` | a `multitrack` | `clips`/`lanes` |

```ts
const editor = await gui.edit(curve, { sampleRate: 48_000, stage: element });

curve.toPoints();      // the edited curve, out of the object you already held
```

**It opens.** The window is up and listening when that resolves, so the
structure you already hold is the edited one from that moment: read it whenever,
and it says what the hand has left there. `{ open: false }` builds the editor
without a window, for a caller composing one. It is `await`ed where the reference
client's `edit` is not, for the reason `plot` and `View.open` are — resolving the
ambient host may have to boot it.

Nothing is handed back: the object passed in *is* the edited one. A **piece** is
one of them, and what opens is the multitrack editor — the second half of this
chapter is what it edits.

Reading it back **after the hand is done** is `wait`:

```ts
await editor.wait();   // resolves when the window is closed
curve.toPoints();      // the curve, as it was left
```

`editor.closed` is the same question asked without waiting, and `editor.close()`
closes the window from the page — the history is not closed with it, since an
undo order belongs to the data.

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

## The view it had, and where the multitrack went

The `form` namespace had a multitrack editor projected out of it, and it does not
any more: `FormEditor` was removed on 2026-09-06 in both clients, with its
examples and this chapter's pages about it. The namespace itself stays,
self-contained, as the data structures described above — it simply has no view.

The reason is structural rather than a defect count. A multitrack's own state —
which track a thing is on, its order within the track, its placement and its
identity — is **authored, durable and undoable**, and a projection has nowhere to
keep it: it ended up in the widget tree, which is drawn, and drawing frees.

What replaces it is a **session** in the same document crate the rest of this
chapter is about — source, region, playlist, track, automation — with three
classic applications built over it: an audio editor, a multitrack editor and a
score editor, each programmable from the GUI host and driven identically from
every client. `crates/clausters-document/PLAN.md` carries that design.

**What did not change** is everything below: the document, where undo lives, and
what a saved session is. Those are the crate's, they were never `form`'s, and
they are what the three applications are built on.


## The arrangement: tracks, lanes, regions

Everything above is the general tree. **Beside** it there is the model a
multitrack editor actually edits, and it is the one the three classic
applications are built over — the audio editor, the multitrack editor and the
score editor, over one document.

The vocabulary is the field's own:

- A **source** is samples. It lives outside the arrangement — the session's
  table says where — and is never overwritten.
- A **region** is one placed thing: a span of the timeline (where it starts, how
  long, its fades, which of the overlapping ones is on top) plus what fills it.
  Six regions over one source are six identities and one source, referenced
  rather than copied. That is the whole of non-destructive editing.
- A **lane** is one of a track's several contents, an ordered list of regions.
- A **track** holds several lanes and **plays one**, which is what comping is:
  record six passes into six lanes, then take from each.
- An **automation** is a curve over one parameter, in the arrangement's time.
- The **arrangement** is the tracks plus what the piece has one of: the tempo
  map, the meter map, the markers, the loop. They are there and not on a track
  precisely so that no two tracks can disagree about them.

```javascript
import { Arrangement, Content, Lane, Region, Tempo, Track } from "clausters";

const piece = new Arrangement();
piece.setTempo(new Tempo({ at: 0, bpm: 96 }));

const drums = new Track({ id: 1, name: "drums", lanes: [new Lane({ id: 2 })] });
drums.activeLane.place(new Region({
    id: 3, position: 0, length: 4, content: Content.onto(take),
}));
piece.tracks.push(drums);

const written = piece.write();   // the crate's JSON
Arrangement.read(written);
```

A **region** is the model's word and a **clip** is the picture's: a clip, a lane
row, a waveform are what the host draws; a region is what an edit names. And
everything placed is placed in **beats**, while what fills a region is measured
in its own source's units — the two are not the same axis, and the conversion
between them needs the tempo map, which is why the map is part of the piece.

### Editing a piece: the verbs a multitrack admits

The piece has an edit vocabulary of its own, and it is reached through the same
door every other structure is — `domainEdit`, with `ARRANGEMENT` as the
vocabulary. Hand over the piece as the crate's JSON and the edit; take back the
piece as it now stands and the edit that puts it back.

```javascript
import { Arrangement, document as doc } from "clausters";

const edited = doc.domainEdit(doc.ARRANGEMENT, piece.write(), {
    intent: "placeregion", region: 3, track: 1, lane: 2,
    position: 16, layer: 0,
});
edited.applied;                     // true
Arrangement.read(edited.state);     // the piece with the region moved
edited.current;                     // the edit that puts it back
```

Fourteen verbs, in three groups. What a **track** is: `settracks` (the tracks
now, whole — adding, removing and reordering are one verb, because all three
say the same thing) and `setactivelane`, which is comping's one verb. What a
**region** is: `setlane` (a lane's regions, whole), `placeregion`, `trimregion`,
`splitregion`, `joinregions` and `faderegion`. What the **piece** holds:
`setautomation`, `setmarker`, `removemarker`, `setrange` (the loop or the punch
span), `settempomap` and `setmetermap`.

Three things about them are worth knowing before you write against them.

**Moving a region to another track is one edit.** Where a region is means track,
lane *and* beat, and an intent is absolute — so `placeregion` states all three
together. One entry in a history, one undo, and no moment in between where the
region is on no lane at all.

**A crossfade is two fades over an overlap**, not a third object: `faderegion`
on each of the two regions, which is what the model already holds. There is no
crossfade to lose track of, and nothing to keep in step with the two fades.

**A split and a join ask you for the content.** They are the two edits that
change how many regions there are, and the two the crate will not work out on
its own: the cut is on the musical axis and a window into a source is on the
content's, and this document converts between the two *never* — that is the
whole reason the tempo map is the piece's. So `splitregion` takes
`left_content` and `right_content` and `joinregions` takes `content`, from you,
who has the map. Omit them and both halves go on reading what the region read.
Both also invert as `setlane`, the lane's previous contents: nothing smaller
describes putting back a region that was made out of two.

Everything else the vocabulary does it does the way the tree's does — absolute,
idempotent, refused rather than merged when it was made against a piece that has
moved. The piece carries **its own version** for exactly that: an editor of the
piece is not editing the tree, so one counter for both would make every edit to
either look like a change to both.

## Editing a piece: `edit(piece)`, and what it plays

A `Multitrack` opens with the same verb the others do, and what opens is the
multitrack **editor**: one widget drawing its own ruler, its own track headers,
its own automation rows and its own boxes, over the same picture and the same
reading of a gesture the standalone host uses.

```ts
await session.gui();
const editor = await gui.edit(piece, {
    sampleRate: 48_000, server, sources: { 1: take, 2: other },
    title: "piece", stage: element,
});
```

`sources` is the one fact about a piece that is not in the piece: the document
names a **source id**, never a path and never a buffer number, so which buffer
each source was read into travels beside it. The same table answers what a box
opens as when it is entered — double click one and its take opens in the sample
editor, on the piece's own undo order.

**Given a `server`, the piece sounds.** The editor keeps one resident reader per
box in a group the server's transport governs, and puts them where the piece says
on every edit whoever made it — this window's gesture, a second window over the
same piece, or a step of the history. Moving a box while it plays is one
`/node_set` on a node that is already running, so it is heard where it was
dropped with nothing that is sounding cut. The window carries the transport row
that goes with it (play/pause, stop, and where the piece is), and `editor.play()`,
`pause()` and `stop()` are the same three verbs from a page. A piece opened with
no server still edits; it is simply not heard.

Two cursors, and only one of them is placed: a click on the ruler — or on the
slack between boxes — puts the **position cursor** down, which is where the next
play starts, and a stopped transport is cued there. The playhead is never placed,
so stop goes back to the mark rather than to the top.

**A curve is drawn and not yet heard.** A track's level, its mute and its solo
reach the readers; the automation drawn on a track and the envelope drawn inside
a box do not, because a curve's `gain` and the header knob's `gain` have to name
one parameter of one node first. Until that lands the curves are edited, undone
and saved with the piece like everything else.

`examples/editors/edit-multitrack.html` is the whole of it, by ear and by eye.

## The presentation: what a window shows, beside what the piece is

Where a window is looking, how far it is zoomed, what the hand is holding, how
tall each track is drawn — none of that is what the piece *is*, and all of it is
state a person loses on a reopen unless something writes it down.

So it is a **`View`**, and it sits **beside** the model rather than inside it.
The shape is Live's and it is deliberate: `Song.View`, `Track.View` and
`Application.View` are objects parallel to their model objects rather than
children, presentation on one side and functional data on the other, both
readable and writable from a script. A `TrackView` is therefore looked up by the
track's id, and an `Multitrack` round-trips the same whether or not a view of it
exists.

```javascript
import { Session, Span, View } from "clausters";

const window = new View();
window.name = "arranger";
window.visible = new Span(0, 48);
window.quant = 4;
window.trackView(10).height = 96;
window.trackView(10).lanesShown = true;      // comping open
window.selected = [20, 32];

const session = new Session();
session.arrangement = piece;
session.views = [window];
```

**There is more than one of them.** A piece drawn in two windows has two views
and they disagree on purpose — the arranger snapping to a bar, the editor below
it to a sixteenth — which is why a session carries a list rather than a view.

**A view entry for something the piece no longer holds is dropped.** `prune`
does it, and the rule is the one this project already fixed a class of defects
by adopting: state goes when the thing goes. Keeping it is worse than losing it,
because a height kept for a track that is not the same track is a defect that
looks like a feature.

```javascript
window.prune(piece);     // true when something went
```

**What a view never reaches.** Not the document, and not the history: a view is
not edited through an intent, an undo never puts a scroll back, and nothing here
is consulted when an edit is applied. A session file may carry one because
reopening a piece into the window it was left in is what every program in the
field does — and a reader that ignores the field opens exactly the same music.

## The document: what the composition *is*

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the Python client, and a GUI host running standalone with
no language attached at all.

**`form` has no door to it, and that is deliberate.** It had one until
2026-09-06 — a bridge that converted its elements to the crate's JSON — and it
was removed with the turn that made the arrangement a model of its own. What a
piece is written with now is `arrangement`, above; what the crate's own document
holds is a **leaf as an id, a kind and a configuration it never interprets**, and
a generator travels as a *reference* the way a project file references a plugin
rather than serializing it. A generator *is code*, in the language that wrote it,
so no format owns one; what the document guarantees is that it does not lose it.

A leaf whose reference nothing here can resolve is **frozen** — it draws, it
holds its place, and it makes no sound. That is the floor, not a failure: it is
what a composition means where the language that wrote it is not running.

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
const session = new Session();
session.arrangement = piece;
session.provenance = { page: "song.html" };
session.sources.set(7, Source.file("takes/vocal.wav", "external"));

const written = session.write();
const reopened = Session.read(written);
```

A source's **lifetime** is what makes saving honest: `external` is the user's own
file, which is never written; `session` is persisted beside the document;
`temporary` is a destructive edit's working copy. A session whose table does not
cover its own piece is what `session.dangling()` reports, rather than reopening
with a take that draws nothing and nothing saying why.

`provenance` is a reference to whatever produced something, carried and never
interpreted. It is what makes re-generating possible without the format knowing
how, which is the same rule the opaque generator follows one level down.

Three questions a save asks the table, and each has an answer rather than an
exception: `session.volatile()` is what is not written down anywhere,
`session.openEdits()` is what is still undecided, and `session.dangling()` is
what the piece names and the table does not hold — **every** lane walked, not
only the ones that play, because an alternate take names its source whether or
not anyone has chosen it yet.

A buffer read from a file knows its `path` and is written as that file; one
allocated in this run is written **volatile** — it existed only while the page
did, and a session that promised otherwise would reopen with silence where it
promised samples. A path inside the session's own folder is written relative, so
the pair of files moves together; one outside it stays absolute, because a
session never claims to own the user's file.

## Reopening: structures, not a description

`Session.read` gives the piece and its table back, and by itself that is half a
verb: every take is a bare source number and nothing has loaded it. Resolving
the table is the other half, and it is the caller's, because what a source *is*
in a running system — a buffer to allocate, a file to map — is not the
document's to decide:

```ts
const session = Session.read(saved);
const buffers = new Map();
for (const [id, source] of session.sources) {
    if (source.path) buffers.set(id, await Buffer.read(`${folder}/${source.path}`));
}
```

Each file the table names is read onto the server **once per source** — two clips
over one take are two windows onto one buffer, and reading it twice gives them
two buffers that drift apart on the first edit. A *volatile* source comes back
frozen rather than as a lie. A file that has moved comes back frozen too, and the
rest of the piece opens: half a session is worth opening. And a generator whose
reference `defs` does not have keeps what it last **rendered** as its floor,
which is the same thing a host with no language attached shows.

Reading a file is asynchronous here and not in the Python client (a page's
`Buffer.read` goes to the worker that owns the filesystem), so the `await` is the
language's and not a different call.

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

## What a piece is as nodes: the channel strip, three times

A piece is a picture and a sound, and this is the second one. What plays it is
not a driver a script writes: it is three GraphDefs over **one** shape, the
channel strip every fixed-channel mixer has had for fifty years.

```text
in -> [pre-fader inserts] -> fader (+ mute) -> pan/width -> [post inserts] -> out
```

A **clip** is that strip over its readers, a **track** is that strip over its
clips, and the **master** is that strip over its tracks. A clip's gain and a
track's gain are both real and they are different stages -- the first corrects
the take, the second mixes it -- which is why an envelope on a clip is not
another name for the track's fader.

The pieces of it are named once, in `clausters_core::mixer`, and both clients
bind the same names: `gain`, `pan`, `width`, `mute`, and a box's own `at`,
`span` and `start`. **The knob in the header, the automation curve and a
`/node_set` all write the same port of the same instance** -- that is what makes
an automation whose target says `gain` drive the gain the header shows, rather
than a second thing spelled the same.

Two words are worth keeping apart, because confusing them is the classic mixer
bug and `pan` is one name for both:

- Over a **mono** source, `pan` is a pan: an equal-power law that puts about
  -3 dB on each side at the centre. The same signal sent to both sides instead
  would be 3 dB too loud in the middle.
- Over a **stereo** source, `pan` is a **balance**: it attenuates one side and
  leaves the centre untouched. A pan law here would take 3 dB off every strip,
  and there are three of them in a row.

Which one applies follows from the source's width -- `Track.channels` and
`Multitrack.channels`, both fields of the document, because the width decides
the mix and reopening a piece has to give back the mix it was left with.
A track's fader is `Track.level`, a field for the same reason: what a piece
sounds like is the piece's, not a key one client reads out of a table it was
only meant to carry.

A strip writes an output bus of its own and a **send** carries it onward at a
gain -- one node between a track and the master. That is what lets a meter mean
something: every track writes into the master's mix, so a meter there would read
the sum and call it the track. A meter is a **slot** on a strip, so a piece
nobody is looking at holds none, and each one writes one control bus per channel
-- one number a block, with the fall and the peak hold applied on the server, so
two clients cannot draw two different falls off one signal.

And the transport plays the piece rather than a client stepping it: each reader
reads the position the engine publishes, so moving a box is one `set` and a
locate is no message at all.

## A cut that plays as one reader

A box is a window onto a buffer, and a cut assembled from several takes -- or
from one take in another order -- is not that. `Buffer.stitch` installs a
**join**: a buffer whose samples are spans of other buffers, read as one.

```typescript
const cut = await Buffer.stitch(
    [{ source: verse, start: 0, frames: 2 * 48000 },
     { source: chorus, start: 4800, frames: 48000 }],
    { server },
);
```

Without it a reader would have to change which buffer it reads with sample
accuracy, and the buffer a reader reads is an initial-rate control: every seam
would be a new node and a control message in the middle of playback -- which is
exactly what a piece that plays itself from the transport must not need. The
fades on a part are the few milliseconds an editor puts on a cut; a source is
held for as long as the join exists, so freeing a take something was cut from
does not silence it.

A join **owns no samples**, so everything that writes into it refuses: writing
would mean writing through to whichever take a frame lands on, which is one edit
becoming an edit of several. Nothing is lost by that -- a join is *replaced*
rather than edited, and re-cutting one costs the list of parts and not the
samples. `take.parts()` is how a view asks before it offers an editable
waveform: the parts when it is a join, an empty list when the buffer owns what
it holds.

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
