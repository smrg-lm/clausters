# The document and the applications

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## The multitrack editor: beats in the data, samples on the axis, and one converter

The arrangement places elements in **beats**; the multitrack view places
clips in **timeline samples**. Two units, and the temptation is to pick one. Both
are load-bearing:

- **The view must be in samples.** A clip's body *is* audio data, and the
  placement work established that a member's data sample 0 sits at its `offset`.
  A take placed at its own frame count then lands 1:1 on the axis, and its
  waveform draws where it sounds. In beats, every body would need a scale factor
  that only the client knows.
- **The arrangement must be in beats.** Its elements are musical, tempo-relative,
  and they render onto a beat clock. Baking a sample rate into them would tie a
  composition to one engine.

**Decision:** keep both, and make the **editor driver the only converter** — one
beat is `sample_rate / tempo` timeline units. It is also where a *musical* `quant`
becomes the lane's pixel-drag grid, so the grid a clip is dropped on is the grid
the arrangement re-schedules on. The arithmetic itself is the core's
(`beats_to_secs` → `secs_to_samples`), never a second implementation, so a port
inherits it.

**Consequence:** `clausters.form` stays pure and transport-agnostic (it is the
piece a future TypeScript client factors into `clausters-core`), the host stays
unit-free (it only knows "timeline units"), and the whole conversion is a handful
of lines in one client module. Its dependency arrow is one-way — the editor
imports the arrangement, never the reverse — the same call that moved the envelope
point helpers out of the GUI submodule so `seq` would not depend on it.

**Amended once the claim was read back: a length is not an onset, and only one
of them is musical.** "The arrangement is in beats" was true of *placement* and
false of *length*, and storing a length in beats is a claim about it — the number
never changes, so what changes when the tempo does is the seconds it is worth.
That is exactly right for a note and false for anything whose seconds were fixed
before the model saw it: a take's length is `frames / sample_rate`, and at double
tempo an event freed after that many beats cuts the recording in half. Measured
before it was changed, on a 96 000-frame take at 48 kHz: `dur` 2.0 sounds its two
seconds at tempo 1 and is freed after one second at tempo 2 — half the take.

So the rule is two rules, and the body says which applies: **an onset is in the
unit of what contains it** (beats, always, here) and **a duration is in the unit
of the element's own data** — seconds for a `Vector`, a `Segments` and a curve,
beats for a `Clang`, a `Sequence` and a `Track`. Nothing new is stored;
`duration_unit` derives it from the body, in the document crate and in both
clients alike.

Two things follow, and both are where the old single-unit claim had put
something. **The conversion belongs to the flattening**, not to the tree:
`flatten` takes the clock's tempo and converts as it lays each item down, because
a timeline is a list ordered by *one* number and cannot hold two bases — which
leaves `TempoClock`, `Playhead`, `Event.dur`, patterns and the editor's ruler and
snap exactly where they were, in beats. And **the converter holds two ratios
rather than one**: `units_per_beat` (`sample_rate / tempo`) for an onset,
`units_per_second` (the rate) for a length in seconds. A take is then drawn as
wide as it sounds at any tempo, and the grid still catches only what is musical —
which is also why the crate's `Place` snaps an offset and leaves a take's seconds
alone.

## A buffer sounds through an instrument, not by itself

Rendering a `Buffer` element was deferred with a note that it "needs an
instrument". The temptation on picking it back up was to give the arrangement a
built-in sampler def.

**Decision:** a buffer is **data**, and it sounds through the def *named to play
it* — `Buffer(buf, instrument="take")`, whose event carries the buffer number in a
`buf` control. A buffer with no instrument is still a perfectly good element: it
draws in the editor and contributes its extent, but emits no event.

**Consequence:** `clausters.form` ships no DSP and no def of its own (it stays the
client-side structure it claims to be), the instrument stays the user's — any def
that reads a buffer works, at any quality — and the "data vs. process" split the
layer is built on holds at its most tempting exception. The one concession is
practical: such an event sets `legato = 1` so a take sounds its whole length,
where a note's default would cut it short.

## The arrangement model: five primitives, one recursive group

A sequencing layer (a timeline of items, a playhead) is enough to *play* music
and not enough to *compose* it. A composition is an element inside an element — a
phrase inside a section, a take against a melody, a generator that has not been
evaluated yet — and the client had no name for that. The question was what to add.

**Context.** The tempting answer is a bag of editor types: an audio clip, a MIDI
clip, an automation lane, a folder track, each with its own fields, its own view
and its own rendering path. That is how a DAW grows, and it is also why a DAW's
granularity stops where its type list stops: you can edit a clip, but not "the
inside of that clip at the level you happen to care about".

**Decision.** A closed algebra instead of a type list. An **element** is anything
bounded that produces a unit of meaning — in one of two modes, and this is the axis
the layer turns on: **generated** (the rendered thing, editable data) or a
**generator** (the algorithm that renders it). Evaluating the second into the first
is the *change of state* — and it is a compositional act, not an optimization: what
separates the two modes is not data versus process but what can be *done* with
them. A generated element is random-access (an audio file plays backwards, slices,
edits in place); a generator is forward-only (it evaluates, in order, and that is
all). Bouncing is what turns something you can only produce into something you can
manipulate. An element carries only two temporal
properties — an *onset* and a *duration*, either of which may be absent (which
gives it a temporal *character*: a segment, a punctual event, a relative segment,
or a pure abstract context). There are exactly **five** kinds, and each is a thin
adornment over something the client already had, never a reimplementation of it:
an **Event** (parameters in one action), a **List** (strict order, no concrete
time), a **Buffer** (a list at constant time), a **Set** (mixed placement — a
track), and a **Function** (a *process*: server DSP, or a generator of sequences).
The one genuinely new structure is the **Group**: a recursive placement of
elements by offset, in two kinds — *concrete* (a relation in time) and *logical*
(a relation of processing, wired by buses). A group's temporal
relation (successive / simultaneous / mixed) is **derived** from where its members
sit, never declared.

Two consequences of the shape are load-bearing:

- **Rendering is a change of state, not a second engine.** Rendering flattens
  the tree — accumulating nested offsets into absolute beats — into the flat
  timeline the client already plays, bouncing any contained generator in the same
  pass. RT and NRT differ only in the destination, so the offline render stays
  sample-identical to what was heard.
- **The base level is the view, not the data.** How coarse or fine a group is
  shown (a summary rectangle, or its members resolved into lanes) is a property of
  the *look*, so the same structure serves the whole scale from a section to a
  note to a parameter — which is the granularity a type list cannot reach.

**Consequence.** The layer is pure and transport-agnostic: no DSP, no protocol, no
GUI — the piece a future client factors into the shared core. It carries the
temptations too: a `Vector` is data and sounds only through the *instrument* named
to play it, and a logical aggregate emits the bus-wired configuration the server
already expresses (a `GraphDef`) rather than a wiring language of its own. Both
exceptions were resolved in the algebra's favour, and both are recorded above.

## Windows all the way: a cut refers, and what two windows share lives beside the tree

A cut is a **window**, not a rewrite: both halves read the contents they always
read, and lengthening either one brings back what the cut hid. That is why a
split can be undone by a join, and why trimming a take back out gives its frames
again. It held for samples from the start and it did not hold for notes, and the
difference is where the contents live.

**Context.** A vector's samples are *outside* the document — a `SourceRef` names
them and a session's table says where they are — so two windows onto one take are
two references and nothing is copied. A track's notes are *inside* the document,
as nodes, because a note has to be addressable: an intent names it, a log
inverts it. So writing two windows onto one timeline wrote every note twice,
with the same ids in each. The format tolerated it (`duplicate_id` refuses two
*different* nodes under one id, and identical copies are not that), which is
precisely why it was invisible: it stayed consistent while the two halves shared
one object in memory, and came apart on reopening, where each half got a
timeline of its own and the two drifted from the first edit.

**Decision (2026-09-03, the user's, over two alternatives).** Windows all the
way. A window is onto samples **or onto a node** (`SegmentSource`), and the node
it names lives in **`Document::content`** — beside the tree, not in it, because
the tree is *placement* and content is what placements read. One id space for
both halves, so a window and an intent name a node the same way, and the finders
an intent uses reach content as well as the tree.

**The alternatives, and what they cost.** *A split that clones* — each half gets
its own copy of the notes, nothing collides, the crate never changes — was
rejected because it takes the property the whole layer is built on: a cut would
delete rather than hide, dragging a half's edge back out would bring back
nothing, and a join would stop being a split's inverse. *Leaving it* was
rejected because the live model and the written one disagreed, which is the one
disagreement a save cannot survive. The clone is worth having as **its own
verb** — "make this stretch a structure of its own" is a real act, and the
absence of it is what made a split feel like it should copy — and it is written
down as one rather than as the answer to this (`crates/clausters-apps/PLAN.md`,
`X9.1`, since 2026-10-01).

**Consequence.** The client writes a timeline **two elements hold** as content
and each reader as a window naming it; a timeline one element holds is written
exactly as before, because the table is for sharing and not for tracks. Every
document written before this reads back identical: samples are the object a
`SourceRef` already was, a node is `{"node": id}`, and `content` is skipped when
empty. What a join across *different* timelines makes — a run of windows over
several nodes — is now expressible, and is the next thing to build on it.

## A view is configured by what it holds, and the arrangement is not the norm

`clausters.form` is the layer that *places* things in time, and because it was
the last big piece to land it started being read as the centre of the client:
the type a value has **there** was taken as the answer to questions that are not
its. It is not the centre. The structures are — samples, notes, events, curves,
segments — and they exist, are edited and are drawn with no arrangement anywhere
near them (`edit(x)` opens a buffer, a timeline or a curve on its own).

**Decision.** The arrangement is a **secondary, specific module**, and every
visual abstraction is independent of it. A clip, a lane, a roll, a waveform are
**views**, and a view is **configured by what it holds**: the drawing and the
edits it admits are derived from the structure inside it, never from which class
built it and never from where in a tree it sits. The wire has said this since the
model replaced the widget catalog — a `field` carrying a placement *is* a clip,
and no type on it spells "audio clip" or "midi clip" — and the same rule governs
the client side of the seam.

**What it rules out, concretely.** An edit action may not be gated on an
arrangement class. The question a split, a join or a trim asks is about the
contents: *does this have an addressable time axis?* Samples do, a timeline of
notes does, a list of events does, and a run of segments does — so all of them
cut and join, by the same action with the arithmetic of their own unit. A
**generator** is the one thing that genuinely refuses, and even that is not "it
cannot be split" but "not until it is rendered", which is the change of state the
model already names. An `isinstance` against `Vector`/`Segments` standing where
that question belongs is a defect, and it is how one action comes to be written
once per type: exactly the per-door drift the auto-fit, the placement and the
selection each had to be unified out of.

**Consequence.** Where a structure that is *general* was filed under the
arrangement, it moves. `Segment`/`Segments` are what cutting anything on a time
axis produces — a signal, a sequence, a track — so they belong beside the
structures rather than in `form/formelement.py`, and every client binds the same one.
And a picture that layers, collapses or refuses is answering a property of its
contents; when it answers a property of the tree instead, the tree has leaked
into a view.

## A pass ends in the playhead's data, and every view drives one transport

Two facts kept being re-derived by every script that plays into a view.

**When does a pass end?** `Playhead`'s feeder returned when the timeline drained
but left the playhead marked as running, so `playing` kept answering yes forever
after the last item and `position` kept interpolating off the end. Each caller
timed the end itself, differently: the score view compared against the last
note's onset plus its length, the multitrack against the composition's extent,
both polling every frame or two. The fix belongs where the fact already is — the
feeder knows it drained — so the drain records it: `playing` goes False and
`finished` says the end is why, as against a `stop` by hand.

It **records** rather than announces. A callback would fire on the clock thread,
which is the one thread a client must never do work on, and the work a transport
does at the end is exactly the wrong kind (a widget update, a socket write). A
flag read from the script's own loop keeps the clock thread free and costs a
comparison. It is also the scan that ends, not the sound: a loop never finishes,
and the last item keeps sounding for its own length — a playhead schedules
items, it does not wait for them.

**What plays a view?** The transport — play, pause, stop, locate, plus the two
numbers the line is made of — is not about what a view draws. `playhead_at` is
one anchor on the engine's sample clock that the host sweeps from; the static
`playhead` is where a stopped transport sits. Both are the same for a lane, a
piano-roll and an engraved page. So there is **one** `Transport` (now `PlayheadSync`), and a view
contributes exactly one thing to it: the unit its static cursor is placed in
(timeline samples for a lane, score milliseconds for a page). What each view
still owns is what a pass *is* — a render of the arrangement, a timeline built
from the engraved notes — which enters as a callable, not as a subclass.

The alternative, a transport per view, is how a client ends up with three of them
that disagree about the end of a piece — which is precisely what was found:
one of the two copies corrected the anchor for the server's latency and the
other did not, so its line ran early by exactly that much. A port keeps the
shape: the arithmetic is small, but its being in one place is the point.

## One document, and sources nothing overwrites

Three parties touch a composition's data and each is good at a different thing.
The **GUI host** has the hand: it draws, it hit-tests, it knows where the
pointer is. The **client** has the algorithms: it generates, it iterates, it
scripts. The **server** has the samples: buffers, and the audio thread that
reads them. Giving any two of them a copy of the model is how the three drift,
and the drift is silent — a note springs back, a clip lands half a grid step
from where it was dropped, an undo writes a state nobody was ever in.

MVC does not settle this, and saying so is the useful part: the pattern assumes
the view *displays*, and every one of these views **edits**. Once a view edits,
"the model" stops being one thing and starts being whichever copy the last
gesture happened to touch.

What settles it is splitting four layers that were being called one:

- **Sources** — the samples. Never overwritten, ever: not the user's file, not
  the session's own. A destructive edit is not an exception but the clearest
  case, because it writes a *temporary* source of its own and the composition
  takes the result only when the edit is confirmed.
- **The document** — the description of what plays when. This is the only thing
  that is "the model", and calling anything else that is what disfigures the
  pattern. Undo lives here, beside the data it inverts.
- **Presentation** — waveforms, peak pyramids, engraved pages, spectrograms. All
  derived, all **invalidated rather than synchronized**: a derived thing that is
  kept in step is a second model with extra steps.
- **Screen state** — zoom, scroll, a selection in flight, a pending overlay.
  Never persisted, never logged.

This is what every audio editor already does and none of them writes down as
such: a DAW's regions reference source files nobody rewrites, and a destructive
sample editor makes the source cheap to replace rather than making the edit
destructive in place. Our own wire had already stated half of it —
`/buffer_setRange` keeps a buffer's shape and fails rather than clamping past
the end — without anyone noticing it was a rule about ownership.

The document is a **Rust crate** (`crates/clausters-document`) rather than a
module of the Python client, and the forcing argument is not parity but the
`standalone` host: it edits with no language attached, so either the model
exists somewhere it can reach or there are two implementations of it in two
languages. The clients then **round-trip the format** rather than holding
handles into a Rust object graph — one function across the ABI instead of an
accessor per field of a tree — and what makes that safe is the crate's central
discipline: it is the only thing that applies an edit. A client does not apply
and then report; it hands over the document and the intent and receives the new
document plus the outcome.

## An id names one node, and a repeated one is refused only when the two differ

A node id is minted by whoever writes a document and every intent addresses a
node by it. Two nodes carrying one id is therefore not a cosmetic defect: the
crate's lookup applies the intent to the first match while the client that sent
it keeps the last in its own index, so one gesture writes two places and the
thing the hand moved springs back. It was reachable by ordinary authoring, from
a direction nobody had looked at — ids are stamped on the element object and
numbering starts at 1 for every root, so two arrangements built in one script
both hold 1, 2, 3, and samples authored in one and used in the other arrives
carrying a number a different element there already holds.

**The fix is in two places because the failure has two sources.** The Python
bridge cannot produce it any more: the conversion **claims** each id for the
object it first meets carrying it, and an object that turns up with an id
already claimed by another is renumbered past everything in the tree. The crate
refuses it at the door — checked on **deserialization**, which is the one point
every writer passes through, so a client, a host and a file all get the same
answer without any of them remembering to ask.

**What is deliberately not refused is one element placed twice.** That produces
a repeated id too, and refusing it would have settled a question that is open
and is not this one: `Group([(0, take), (4, take)])` is the arrangement's most
natural sentence for *this take, twice*, and what an id identifies in that case
has three candidate answers — forbid it, copy it, or have the intent name the
**placement** rather than the node. One of those three is "forbid", which is
exactly what a validation that refused every repeated id would have chosen, from
inside a check about a different failure.

So the line is drawn by **what the two nodes are**, not by the fact that they
repeat: identical nodes are *ambiguous but consistent* — the document says the
same thing twice, and which placement an intent means is the open question —
while different nodes are *incoherent*, since no answer to that question makes a
document well-formed in which one id names two different things. The message
says which two it found (`node id 2 names two different nodes, an aggregate
and a clang`), because the id alone does not tell an author where to look.

**One asymmetry, recorded rather than smoothed over.** The C ABI's
`clausters_document_open` answers with a null handle and has no channel for the
crate's message, while the wasm face raises it as an ordinary error. So the
Python client looks for the collision itself once the handle comes back null,
and only then — nothing is validated twice on the path where the document is
fine.

## A node carries a name, and an aggregate carries the restrictions its writer put on it

A session did not round-trip. A piece authored with named lanes and a track in
each reopened anonymous, with a level of nesting nobody wrote and the rolls gone
— and both halves were the format's rather than any client's, because what was
missing was somewhere to put them.

**The name is the server's rule, taken rather than invented.** A node gets an
optional `name`, and it is a *referenceable label, never a second identity* —
which is exactly what `/group_new`'s name already is on the wire: the id remains
what every intent addresses and every outcome reports, and the name is a second
way to refer to the same thing. A node is born named or stays anonymous, nothing
addresses by name, so an anonymous one is reachable exactly as before. The
alternative — keeping the label in each client — is what had been happening, and
it means a piece labelled in one writer opens unlabelled in the next.

**The track is the harder half, and the answer is that there is still one
aggregate kind.** A multitrack's track is *an aggregate with the restrictions of
a view*, and the
layer's own rule says the tree stays general — no lane, no vertical position, no
type per container — because a view is a projection and may decline what its
shape does not admit. But a writer that has such an aggregate has to get it back, or
the format silently promotes every track to a plain aggregate. So the **restriction
travels as opaque configuration**, through the same door a generator's code goes
through: `Body::Aggregate` gains a `config` the document carries and never reads, the
Python client writes `{"form": "track"}` into it, and the crate is not one line
wiser about what a track is.

That line is worth stating because it looks like a loophole and is not. Carrying
a payload is what this format already does with everything it cannot own; what
the rule forbids is the tree *acting* on view-ness — sorting by it, validating
against it, giving it a variant of its own. A reader that does not know the key
lays the set out as a set, which is precisely the behaviour an unknown widget
gets in the GUI protocol.

**What it does not fix**, said so the next reader does not assume it: the two
clients now have to agree on a string that no schema checks. That is the cost of
keeping it out of the model, and it is the same cost the GUI protocol pays for
`kind`. The alternative — a typed `view` field — would be checked, and would put a
view in the tree, which is the trade this layer decided in the other
direction on the day it was designed.

## The working copy leads, and it is the buffer the samples were loaded into

O8 says the working buffer leads while a session is open and that a take's pool
buffer is replaced whole once, on confirmation. That was not a preference: it
was derived from a measurement — a write cost the whole buffer (33.8 ms on a
five-minute take), so an editor that wrote through per gesture was unusable, and
a client-side working copy was the honest workaround. S18 removed the
measurement: a write is now flat in the samples.

**The rule survives the measurement that motivated it, and what dies is a
reading of it.** The server buffer *is* the working copy. Loading samples into
a pool buffer copies it — `/buffer_allocRead` reads a file and never touches it
again — so an edit that writes that buffer has already not written the source.
What the four-layer table protects is the user's own file, and the copy that
loading made is what protects it. There is therefore **no second take** while
one is edited, and **no confirmation step per edit**: what confirms a stroke is
the acknowledgement and the log entry, which is what the editor already does.

**Undo never needed the copy either**, and the crate had already said so.
`inverse_of` returns the *empty* write for `WriteSamples` — the samples are not
in the document — "which is why a destructive caller reads its own span before
writing", and the host does exactly that: the previous samples ride in the log
entry, span by span. A second take would have held the whole samples to
recover what a stroke covers.

**Where a temporary copy stays mandatory** is a property of how the samples are
held, not of what a write costs: samples reached **by reference to the user's
file** — mapped rather than loaded, which is the path shared-memory editing
opens. An edit there would write the user's file, which the four-layer rule
forbids outright, so it must write a `Temporary` copy first, and
`confirm`/`promote` are what settle it. That is the whole remaining job of the
`editing` field in the session format, and why it stays: a session that dies
mid-edit over mapped samples must reopen knowing what was undecided.

Stated as one line, because the distinction is easy to lose: **an edit writes
the copy the system already made, and makes a copy only when it has not made one
yet.**

## A redo reports what it applied, so it is the same shape as an undo

An undo hands back the intents it applied and the caller projects them onto
whatever it draws. A redo did not: it applied its steps to the document inside
the crate and answered only with what it *could not* perform, so a client had to
take the whole document and walk it against its own objects to find what moved.

That asymmetry cost three things, and the third is why it is worth a record.
It was **O(document) per step**, in a design whose whole point is that an edit
costs the edit. It was a **second implementation of what an edit means** —
reconciliation, on the one path that had no reason to need it. And it was the
cause of a defect that read as a dead button: only one of the two routes kept
the *drawn record* in step, so a redo moved the model and told the host to go on
drawing the clip where it had been, and the picture caught up one step late, on
the next undo.

So the log's redo now returns `redone` beside `remaining`: the intents it
applied, in order. A client projects them exactly as it projects an undo, and
the adopt-the-whole-document path is **deleted** rather than extended — which is
the opposite of what an earlier attempt at undo-inside-a-clip had to do, and the
reason that attempt was reverted.

`remaining` keeps its own job unchanged: the steps from the first one the crate
cannot perform onward, for the owner to re-run.

## A node id names the placement, and what may be placed twice is what a node references

`Group([(0, take), (4, take)])` is the arrangement's most natural way to write
*this take, twice*, and it silently mis-addressed every edit: the conversion
stamped the node id on the **element object**, so both placements wrote members
carrying one number, the crate applied an intent to the first match and the
editor's index kept the last. Two writes, two destinations, and the clip the
hand moved came back to where it was.

Three answers were on the table for months — forbid it, copy it, or have the
intent name the **placement** — with the third recorded as the faithful one and
the most expensive, "because a member has no stable identity in the document".
Read against what a multitrack *is*, the first two are not options: a clip is a
**window onto samples** and the identity is the samples, so forbidding the
sentence outlaws non-destructive editing's basic move, and copying quietly forks
what the author wrote as one thing.

**The third turned out to cost nothing, and that is the finding.** A `Place`
already names a node, and a node is already what a `Member` holds — the format
has been able to express two windows all along. What collapsed them was the
bridge. Moving the stamp to the **member handle**, which `Group.add` has always
returned and whose docstring already called it "the stable identity `remove` and
`move` take", makes each placement its own node with no change to the crate, the
wire, or the intent vocabulary. It was expensive against a model where a member
has no identity; that stopped being true the day the handle existed.

**What that leaves is the real question, and it is about samples rather than
addressing.** Two windows share samples only when the node *references* it. So
a **buffer** (two nodes, one source), a **generator** and a pattern-backed
**sequence** may be placed twice — two views of one take, two evaluations of one
function, which is the instance/function distinction the user's own exposition
named. An **event**, a **track** or a **group** carries its samples *inside* the
node, so a second placement is a second copy that diverges on the first edit;
that is refused with the distinction rather than made in silence.

What stays open is the **alias** — a node that says "my samples are that node's" —
which is what would let a container be placed twice and edited in one place, and
a placement's own **arguments**, for a function evaluated twice with different
parameters. Both need a placement to be a thing with an identity, which is what
this makes it.

## A clip is a window onto a segment of data, so its edges trim it

**Context.** A clip's take was fitted to whatever rectangle the clip had, so
shortening a clip squeezed its picture — while playback kept reading the buffer
from its first frame for as long as the event lasted. The picture said one
thing and the sound another, and what the picture said was *time stretch*, which
is a rendering nobody had asked for and which this project does not implement.

**Decision.** The multitrack model: a clip is a **window onto a segment of its
samples**, the memory-view idea. `SourceWindow` says where a placement's own
time zero reads (`start`), whether the window wraps (`loop`), and — as an
explicit opt-in — whether the picture is fitted instead (`fit`, the prop a time
stretch will set when there is one). **One timeline sample is one source
frame**: trimming hides frames rather than compressing them, and opening the
window again brings them back.

Everything the definition of a clip carries falls out of that one property:

- **The start grip is a trim**: the offset, the duration and the window's head
  advance together, so the samples stand still while the clip shows less of
  it.
- **The edges stop where the samples end**, unless the clip loops — where past
  the end is the beginning again and before frame zero is the tail of the
  iteration before, which is what stretching an edge means on a loop.
- **A split is two windows over one source**, and a **join** is the inverse; the
  frames neither half shows are still there, which is why a join can put back
  exactly what a split cut.

**Consequence.** A take is drawn a **run at a time**, one run per pass over the
samples: the wrap lives in the run list rather than in the coordinate maps, so
one affine renderer draws a looping clip, a plain one, and the part of a clip
that reaches past samples it does not loop — which stops there instead of
clamping into a flat line nobody recorded.

The window travels with the axis (`TimeSpace`) because it is the other half of
the same mapping: the container decides which part of the data its time covers,
and the element is told rather than working it out.

**On the client side a trim is one edit, not two.** Where a clip sits is its
placement's and what it reads is its element's, so a trim touches both — and a
gesture recorded as two log entries takes two undos to reverse, the first of
which leaves a clip showing frames it does not play. A parent's members carry
both (a member is a placement *and* the node it holds), so one `SetMembers`
states the result of the whole gesture.

**What the arrangement cannot express**, stated because the join meets it: an
element reading **several segments** — two different files, or two windows with
a gap between them, read as one thing. An element wraps one thing. Such a join
is refused by name rather than approximated, because approximating it silently
drops samples.

## The arrangement's primitives are named for what they are, not for what they wrap

**Problem.** Three of the arrangement's primitives carried names another part of
the project had already taken, and two of them had been taken by the very object
the element wraps. `clausters.form.Buffer` wrapped a `clausters.defs.Buffer`, so
the sentence describing it was circular and the two could not both be imported
into one script. `clausters.form.Event` wrapped a `clausters.seq.Event`, and the
editor had to alias one of them at every call site (`Event as FormEvent` beside
`Event as SeqEvent`) to keep them apart. `clausters.form.Group` was not the
server's `Group` at all — that one is scsynth's node-tree group, named by every
`/group_*` command and every reply — and prose about "a group" had to say which
one it meant every time.

Shadowing is the visible cost, but it is the smaller one. The larger is that a
name repeated across two layers stops carrying information: a reader who sees
`Buffer` learns nothing about whether they are holding samples or a placement
of it.

**Decision.** The three are renamed for what they are, and the rename reaches
every end of every wire in one move — the Python classes, the `Body` variants of
`clausters-document`, and the `kind` strings of the saved format:

| was | is | what it is |
|---|---|---|
| `Event` / `Body::Event` / `"event"` | `Clang` / `Body::Clang` / `"clang"` | parameters or actions that happen together, internally simultaneous |
| `Buffer` / `Body::Buffer` / `"buffer"` | `Vector` / `Body::Vector` / `"vector"` | a succession of data at constant rate |
| `Group` / `Body::Set` / `"set"` | `Aggregate` / `Body::Aggregate` / `"aggregate"` | the recursive container of placed members |

**Why these words.** `Vector` was already the crate's own gloss for the body
(*"a succession of data at constant rate: a vector"*), so it is adopted rather
than invented. `Aggregate` names the container without `Set`'s collision with
Python's builtin, and the arrangement's is the one that moves because the
server's `Group` is scsynth's and is what the protocol already calls that thing.
`Clang` is James Tenney's term, from *Meta+Hodos*: a **gestalt unit**, a
sound-configuration perceived as a single thing, which he derived from the
German *Klang*. That is precisely what this element is, and it is the one name
here that names the object rather than describing it.

**Two words it is not, and neither is an objection.** SuperCollider has
`Klang`/`Klank` UGens (banks of sines and resonators) — a different spelling, a
different layer, and Clausters ships neither, having declined to port scsynth's
UGen catalog wholesale. And `clang` is the C compiler this repo's build
instructions install for bindgen — a different domain entirely, never a type in
any namespace of ours. A word can belong to more than one domain; what it may
not do is name two things in the same one, which is the whole reason these three
moved.

**Consequences.** The saved format changed, and there is no compatibility
shim: a document written before this reads its three renamed bodies as
`Body::Unknown` and round-trips intact but is not understood. That is acceptable
pre-1.0 and would not have been after. The `grouping` field keeps its name, as
do `Sequence`, `Segments`, `Track`, `Generator`, and the `CONCRETE`/`LOGICAL`
kinds. `Vector.to_event()` and `Segments.to_events()` keep theirs too, because
what they return really is a `clausters.seq.Event`.

## An undo history belongs to an editing context, and the context is one pile

An undo stack belongs to the data, not to the view: two windows over one
composition must share a history, or stepping one of them reverts across the
other's edits and writes a state nobody was in. That much was never in question
— the crate placed its log beside the document for exactly that reason. What was
open is the shape one level up, once the requirement stopped being the
arrangement's alone: a client builds a structure of its own (a curve, a buffer, a
roll), edits it in a view and reads it back; and an application composes several
editable views whose history is the interleaving of what was done in each.

Two shapes answer that, and they are not equivalent.

**A pile per structure, plus a counter to merge them.** Each structure keeps its
own linearity, and a view holding several walks them interleaved by a
session-wide sequence number. It is the obvious generalization and it is the one
we rejected. A view that filters one shared order down to the structures *it*
shows is doing **selective undo**: inverting an entry that touched A and B while
a later entry over B still stands. That is precisely the failure the whole
mechanism exists to remove, reintroduced by the mechanism itself. A history is
one order or it is not a history.

**One pile per editing context**, which is what shipped. A `History` holds the
structures registered in it, one ordered list of entries over them, and one
cursor. An independent structure is a history with one structure in it; a
composed application is a history with several, and the interleaved order *is*
the list, with no counter and no merge; two views of one structure are two views
of one history. What decides what shares a history is which history a structure
was registered in — never which window is looking at it — so a structure belongs
to exactly one, and a history refuses an entry naming an identity it did not
mint.

Two consequences worth stating, because both were argued and neither is free:

- **The identity is minted by the history, not carried by the data.** The
  arrangement has node ids only because the client stamps them; a curve a script
  built has none, and inventing a stable one for it would be a second identity
  system for a value that lives as long as a window. The minted handle is also
  the read-back path: the identity that opened an editable view is the one its
  edited state is read out through. It is process-wide rather than per-history,
  because a per-history counter hands the second history's first structure the
  same number as the first's, and the refusal above would then pass on exactly
  the case it exists to catch.
- **An entry's payload is opaque.** One pile over several domains cannot hold
  any domain's type, so a payload is the same JSON the crate already carries a
  leaf's configuration in, and each leg names its structure so a caller can route
  it. The price is that an edit whose bulk follows the audio is held as JSON;
  the ABI boundary already pays that, and it is what buys one pile instead of a
  vocabulary the pile has to know.

## A shared region is a picture of a join, not a replacement for it

The server copies a buffer's samples into a shared-memory region when a peer
can map one, so an editor draws a take with no message at all. A **join**
(`/buffer_stitch`) has no samples of its own to copy, so the copy path read it
sample by sample and installed the flattened result — and that install reached
the engine and the mirror as well as the region, which quietly undid the whole
of what a join is: the sources stopped being named (`/buffer_parts` answered
"not a join"), the memory a join exists not to duplicate was duplicated, and
the reader lost the run it reads a cut by. It was invisible because the
flattened copy *sounds right*: it is the same samples.

So the two are separated. The region gets the samples — a peer wants a picture
of the cut and draws it like any other take — and the server keeps the join.
What makes that safe is a rule that was already there: **a join is read, never
written**, and it is replaced rather than edited, so the picture cannot go
stale under the peer without the replacement it would be drawn from arriving
too.

The general form, worth stating because sharing will reach other shapes: what
is published for a peer to *look at* is a rendering, and what the server keeps
is the thing itself. A publication that replaces its subject is a bug however
equal the two look from outside.

## An application owns a window set, and an editor owns one structure

An editor used to own everything global about a session on screen — the host,
the window, the widget-id pool, the acknowledgement, the undo walk — while what
several views actually share (the history, the version, the views to tell) sat
one level down, in the editing context the *data* owns. Three things followed: a
widget id was a lease rather than an identity, everything global was written
once per language, and there was no layer that meant "an application", so a
bundle of views that is not the multitrack had nowhere to be built.

So the two swapped places. An **application** (`Application`, in both clients)
owns the window set: the host and how it is resolved, the widget-id space, the
socket drain and the wait and close surface. An **editor** is one structure
bound to a domain and a view, and owns nothing global.

Two things stayed where they were, and the reasons are what a later pass would
otherwise undo. The **acknowledgement** is one view's end of the conversation:
the staleness floor rises when the version moved and no event of *this* view
moved it, so two windows sharing a floor would each silence the other's check.
And **screen state** — selection, zoom, which layer the hand is on — is the
host's by the four-layer rule and each window's; what a client keeps is a
read-back value, keyed by the object **weakly**, never by its address: CPython
reuses an address the moment an object is freed (196 times out of 200 in a
straight loop), so a table keyed by `id()` hands its state to whatever lands
there next. The undo order is neither the editor's nor the application's; it is
the editing context's (below).

## One undo order, held by the applications crate's editing context

A piece and a take edited beside it, or two windows over one take, have to walk
one history. The history sat in each client's editing context and in the
standalone host's own log, while the multitrack editor handed its entries back
to whoever kept them — the chaining around one pile written three times.

So `clausters_apps::editing::Editing` holds the history, the version and its
**members**, and no application holds a history of its own: an editor is
opened *in* a context, and one opened alone is a context of one. The context
owns the editors behind one handle; two handles referring to each other would
have left the turn → record → step chaining in every endpoint, and a borrow
across handles that wasm cannot keep. A member declares a **key** — a take by
its buffer, a piece by the piece — and a second member with the same key is the
same structure. Structures the crate does not apply (a client's curve, its
notes, a score, the host's tree) join as **external members** whose legs come
back to be applied.

A turn records the entry and moves the version. A step hands each leg to the
members holding its structure and, when nothing could apply it, puts the cursor
back and says why — a step over an entry nobody holds would otherwise lose that
edit from the order. A take's step comes back as its writes rather than as
steps, because the transport bound a write is chunked by belongs to the server
the take is on. And the C door runs a verb once across its sizing and filling
calls: a history is not copied for a sizing pass the way an editor is.

## A track's contents are a lane, not a playlist

The field's structure for a track's contents is Ardour's **playlist**, inherited
from Pro Tools: an ordered list of regions, several per track, one of them
played — which is what takes, comping and alternate versions are. The structure
is right and the name is not: in ordinary use a playlist is a list of songs, a
word that has to be decoded before it means anything here, which is exactly what
this project's naming rules refuse.

So the structure is Ardour's and the name is ours, **`Lane`**: a track holds
several lanes, and a lane is an ordered list of regions. A track with three lanes
draws as three rows, so the model's word and the view's word are one word about
one thing. And it is the **lane** shape rather than takes inside the item, on
evidence: REAPER kept takes inside the item for twenty years and had to add
*Fixed Item Lanes* on top for real comping, and both models now coexist in it.
The cost, stated when it was decided: `lane` also named a channel row inside a
multichannel clip body in the host, and that sense is the one renamed to
`channel`.

## The multitrack is placed in seconds, and a tempo map is a structure it holds

The original error was measuring **all** time on a base of beats. The document
placed every region, fade, curve point, marker and span in beats, so the piece's
tempo map was the axis of everything in it: every client crossed a position to
frames through the map, an edit of the tempo moved every box, and each plan,
projection and editor had to be handed a `default_bpm` for a piece that stated
none. That is a sequencer's model applied to a multitrack, whose material —
recordings — is measured in physical time.

**Everything a multitrack places is in seconds**, the way the server and the
clients already measure (`timebase::Second`). A position crosses to frames by the
rate alone, so the instance plan, the props, the intake, the playback and the
editors take no tempo, and a tempo edit redraws a ruler and moves nothing.

**That does not stop a document holding structures organized by meter and
tempo.** The tempo and meter maps stay in the document and are saved with it,
stated at beats: they are a structure the document holds, read by a ruler
drawing beats and bars and by a snap to them, rather than its axis. A region
whose contents are in beats keeps them — placed in seconds, the way a child
timeline is placed in its parent — and which map converts them is left open
until something plays such a region. *(Dissolved the same day: no multitrack region
holds beats. The composite region that suggested one is the `form` tree left
inside the multitrack document, and it is filed for removal. Removed
2026-10-07, with the window onto a node of the general document: a region is a
window onto a source, and the type says so.)*

**One unit for tempo.** `TempoClock` and `TempoMap` were in beats per second and
the document in beats per minute, divided by 60 wherever the two met. Every
tempo is now beats per second, the document's included; beats per minute is
presentation.

**Seconds cross to samples by one rule** (found the day it landed, when boxes
that met on a sample stopped meeting). A position lands on a whole sample through
the core's rounding, and a length is the difference of its two ends, each
crossed (`tempoclock::secs_to_samples_over`); where two boxes meet is asked in
samples. A length crossed on its own and an end crossed as a sum disagree in the
last bit, and that bit is a gap. Integer ticks (Ardour's superclock) were weighed
and not taken: seconds are what the server measures in, and the units stay
readable; the rule is what makes them unambiguous.

**An old file is migrated once, in Rust.** Format 3 keeps the field names and
changes what their numbers mean, which is exactly the case the format counter
exists for. `session::migrate` converts a format-2 file through the tempo map it
saved (a length being the difference of two converted positions, a region's own
curve measured from the region's start), and both clients' `Session.read` and
the GUI host call it, so an old session opens the same everywhere.

## A source at another rate is resampled as it plays, and one ratio says so everywhere

A multitrack is placed in **seconds** and drawn in the session's **samples**,
while the samples behind a box are counted in the **source's own frames**. Those
are the same number only while the source was written at the rate the session
runs at, and until now everything assumed they always were: the reader added the
transport's frames to the window's start, the projection crossed the window's
start with the *view's* rate, the picture read the source frame for sample, and
the edge a hand pulls compared a frame count against a length in session
samples. A 44.1 kHz take in a 48 kHz session was therefore drawn **8.8% longer
than its samples**, its edge stopped 8.8% short of them, and it played 8.8%
sharp from the wrong frame.

**The decision, the field's and the user's: nothing is converted on the way in.**
A file keeps the rate it was written at, the buffer keeps it (`/buffer_read`
always has, as scsynth does), and the **reader resamples as it plays** — in real
time, or in a render, through the same nodes. That is what `PlayBuf(buf,
BufRateScale(buf) * rate)` has always meant here, and the multitrack's own
reader was the one player not spelling it.

**One ratio, three consumers.** How many frames of its source one sample of a
box is:

- **The reader that sounds** (`clausters_core::mixer::reader_def`) crosses its
  phase by `BufRateScale(buf) * rate` and its window's start by
  `BufSampleRate(buf)`. Both come off the buffer, so the audio path needs
  nothing said by a client: what travels is the region's `playrate`, the half of
  the reading speed that is a decision rather than a fact about the samples.
  `mixer::START` is therefore in the source's **seconds**, not in frames — a
  plan that crossed it would need every source's rate to state a number the
  server already has.
- **The picture** (`SourceWindow::rate`) maps a box's local time to source
  frames by the same ratio, so what is drawn is as long as what is played.
- **The edge a hand pulls** (`structures::boxes::Contents::rate`) converts the
  frames behind a box into the box's own units before it stops there.

The two clients hand the ratio's other half over in the source table they
already keep (`{buffer, channels, frames, rate}`), and the projection turns it
into the `rates` prop — flat `box rate` pairs, a name list like `loops`, and for
the same reason: it follows from the source and the region, so a hand cannot
edit it and a report carrying it would hand back a number the widget was told.
A box not named reads one frame per sample.

**Zoomed to the sample, the dots are what the engine produces.** A resampled box
reads *between* its source's frames, so drawing the source's frames where they
fall would draw samples nothing plays. The drawing steps the placement's own
grid instead and reads each point the way the reader does — and "the way the
reader does" is a shared function, `resample::Interpolator::played`, which both
`BufRd`/`PlayBuf` and the drawing call: **linear**, because a reader answers in
one multiply-add per sample. The band-limited reconstruction stays where it was,
as the `signal` layer drawn *through* the dots, and where the two visibly part,
that is the resampling error — a true thing to be able to see, and the reason
the two readings are two functions with different names rather than one anybody
could reach for by accident.

**A join takes parts at any rate, by the same rule.** It used to refuse them
(`a join is not a resampler`), and the reason that refusal looked necessary is
that a join seems like a conversion. It is not: a stitched buffer **owns no
samples**, so there is nothing to convert into. Each part carries the ratio its
source's rate makes with the join's and is read through it -- two frames and the
line between them, per sample, for the parts that need it -- which is the same
reading a box over a take at another rate already gets, one level up. A part at
the join's own rate keeps the indexed load it always had, behind a comparison
made once when the stitch is built.

So the ratio never travels: the server takes it off the buffers, as it does for
a reader, and what the command states is how many frames **of the join** a part
fills. That is the one meaning that changes -- a part used to state what it read
of its source, and the two are the same count only at one rate. The picture
follows the same shape: `segments` gained the ratio per span, so a join is drawn
from the takes it is spans of whatever rates they are.

**An audio editor's list reads takes at any rate, by the same rule**
(2026-10-17). The parts arithmetic (`clausters_document::parts`) counted every
part at the list's rate, and so a block copied from a file at another rate
was refused when pasted. Each part now fills the list by the ratio of its
take's rate to the list's, computed by `parts::contributed`, the one function
the stitch also counts with, so the editor's arithmetic and what the server
stitches cannot disagree. A pasted block is written at its own rate and read
at the ratio; nothing is converted on the way in. A span that starts or ends
inside a part at another rate is cut at the nearest frame of its take. A mix
stays one rate: a sum is taken sample for sample, and fitting one block to
the other's rate would be a resampling nobody asked for, so it is refused
with that reason.

What a join is still not is a **converter**: nothing is written out at the
join's rate, so a mixed-rate join costs the reading and not memory. Wanting the
samples themselves converted is rendering them, which is a different verb and
does not exist yet.

**Undo and redo do not change any of this, and that is what settles the
storage.** A join is built once and its samples are its parts'; a take buffer is
freed by nobody on an undo (the reconciliation frees only the curve tables), and
a source the multitrack stops naming is still a source -- the rule that keeps a
redo finding what it left. So the whole history is walked with no conversion
repeated and nothing written to disk, and a join reaches disk only when a save
promotes its lifetime, like any other minted source. The cost that leaves is
**memory**: the takes a history can still reach are held for as long as it can
reach them, and giving them back is tied to discarding history rather than to
the document. That is written down as open in `crates/clausters-document/PLAN.md`
rather than solved here.

## The audio editor's history holds takes, never samples, and its save writes the user's file

*(Decided with the user, 2026-09-22.)* An audio editor edits a take as a **list
of parts** over takes that are never written once a history entry names them: a
cut is a new list, and a paste, a mix or a pencil stroke writes a new take the
size of the span it touches and splices it in. The alternative every editor
starts from -- write the buffer in place and keep what a stroke replaced as the
inverse -- puts samples in the history, and editing a long file is then a
history the size of the edits. Here an entry is two lists and the names of the
takes they read, so an undo costs the list; the takes are given back by
reachability (the document, the history, the clipboard) and, past a resident
budget, written to a scratch directory and read back when a step needs them.

The earlier decision that a copy-on-write per block would rewrite a megabyte
for a fifty-sample stroke, and would have to be flattened to be heard, no longer
held: a take the size of the span is not a block, and the server plays a join
as it is.

**Its save writes over what the take was opened from** -- a file, or a server
buffer. That is the one place the program writes a file the user brought, and it
is deliberate: in an audio editor saving *is* the user's act on the user's file,
and the rule that no file is ever overwritten protects that file from everything
the program does on its own -- an edit, a render, a spill -- not from the save
the user asked for. A save-as writes another file or another buffer instead,
which a later save then writes over. A buffer is rewritten whole at the take's
length, so whatever reads it hears the edit from then on; saving into one that
is sounding is heard as a glitch, and that is the user's to avoid rather than
the editor's to refuse. What makes a save into the opened buffer safe for the
history is that the editor never edits it: it opens a **private copy**, and the
history names only that and the takes the edits made.

The audio editor and the multitrack editor are **separate applications**, with
different purposes: the multitrack edits non-destructively and opens no editor
over what a box holds, and the audio editor has a history of its own. What makes
an application coherent -- its rules of use -- is the application's; what the
crate implements is what each can edit.

## The round-trip rule is annulled: the applications hold the structures

The document crate was bound with a rule: the clients **round-trip the
format** and do not hold handles into a Rust object graph -- one function
across the ABI instead of an accessor per field of a tree. It was decided for
the document's general tree, against the surface an accessor handle costs,
and it was relaxed once already, for cost: a document handle with the same
three verbs the by-value binding had.

**It does not hold for the structures an editor edits in place.** A notes
editor edits the very `EventSequence` a script names, so the sequence became a
handle, and its events and curves then became live objects over it with an
identity map: an object read twice is one object, a read asks the
structure, and a write goes through the structure's vocabulary and, when it
has one, its history. That broke the rule, and nothing said so. The multitrack
follows: a script and a window over one multitrack are two holders of
one tree, not a tree and a copy that has to be written back after every turn.

The reasons are the editor's, not the ABI's. A copy has to be reconciled after
every gesture, and a script's change to it is no turn the window sees -- not
redrawn, not undone by the window's Ctrl+Z. A copy also has to carry the
document's ids into the client's surface, which is how a script came to build a
`Track(id=1)` and look a region up by number. With one tree, the ids stay the
crate's and the objects stand for the structures.

**So the rule is annulled, not relaxed.** It was written for a design in which
a client held the document and the crate was a function it handed the document
to -- apply an intent, answer the new document and the outcome. The
applications changed that design: the editors, the editing context with its
history, and the playback are the shared crate's (`clausters-apps`), and they
hold the structures they edit. A client is then one more holder of a structure
an application holds, and a handle is simply what holding it is; the rule has
nothing left to govern. It is also what makes the standalone host the same
program: it runs the same editors over the same structures with no language
attached, where a design that kept the document in each client would have
had a third copy to keep in step.

What still crosses by value does so because it is **data**, not because a rule
asks for it: a whole multitrack handed to `domain_edit` as JSON, a session file,
and the values a structure is made of -- a fade, a tempo entry, a region's
content.

## A timeline renders to a sequence and holds none

A `Timeline` is a client abstraction: entries placed in beats, some of them
code -- a pattern, a routine -- whose output exists only once it runs. Its
relation to the server is one direction: `render_events` turns it into an
`EventSequence`, the structure the server shares, with what the code yields
rendered as events, and that sequence plays on a transport. Locate, loop and
stop are then the transport's.

The other direction was built once and taken out (2026-10-03): an edited
sequence placed back in a timeline as an entry. To play it there, the
timeline's client-side tree had to place a sequence's beats on a client clock,
re-read it on every edit, and measure it through seconds -- a transport's
position done a second time, outside the transport -- and it still played none
of the sequence's curves, which sound only on the sequence's lane. A proposal
in the same family, a timeline splitting its items between a lane (the data)
and the client's plan (the code), was dropped for the same reason. Nothing that
a transport plays better is a timeline's entry.

What the attempt surfaced is kept on its own merits: an event played from the
client had no way to carry automation. That is solved in the events, so that
whatever plays them -- a pattern, a routine, a timeline -- plays their curves,
not by putting a sequence back in a timeline.

## A key names a verb, and a verb nobody performs is the application's

*(2026-10-04, with the user.)* The keys were `match` arms inside the elements
that perform them and inside each front's window handler, so the same key could
not mean two things in two applications over one widget, a user could not
change one, and the two fronts drifted (the browser's space bar once never
reached the window's transport). They are now one table per host, `verb =
chord(s)`, set like the theme: defaults, `[gui.keys]`, `--keys <file>`,
`/gui_keys`. The user's reason for a **file** first is that an application here
may have no chrome at all -- no bar, no menu -- so the keys are its only way in.

Two calls in it are not obvious. **The table holds any name, not only the
host's verbs**: a name the host does not perform is reported to the window's
owner as the bare verb, which is the shape `undo` and `save` already had -- so
those stopped being special cases and became the application verbs the table
ships bound, and a program with no chrome gets a command by naming it. And **a
key bound to a verb the window's bar names reports that entry's pick** rather
than the bare verb -- the desktop rule that an accelerator and its entry are one
command -- so a program with a bar answers one message whichever way the
command came, and a disabled entry's key does nothing. The host's own verbs go
first (focused element, element under the pointer, window), so an Edit > Copy
entry does not take Ctrl+C away from a roll that performs it; only what nobody
here performed reaches the owner.

**`Ctrl` is the platform's command key**: Command on a Mac, natively and in a
page, for every gesture the host qualifies with Ctrl and not only the keys --
the reading a desktop toolkit gives a portable chord there -- so one table is
right on both platforms instead of each user rewriting it. The key left over
(Control on a Mac, the logo key elsewhere) has no name in the host, and a key
pressed with it reaches nothing rather than arriving as a bare letter.

What stays outside the table: the focused element reads the raw key before the
table is consulted, so a field's typing, a list's arrows and a control's Space
are the platform's convention and cannot be rebound, and Tab and Escape are
refused.

## A box of notes is a clip over a bus its voices write

A track's fader, its gain automation, its mute and its meter did not reach the
notes of a box on it (the user, 2026-10-06): the notes of every box played on
one event lane and each voice wrote the hardware through its own `out`, past
the strips. The rule decided is the user's — **what a track produces is what
its strip shapes, whether a box reads samples or plays notes** — and a box of
notes is therefore a source of sound *inside* its track, as a box of samples
is.

**The voices are not members of the clip.** The obvious shape — the note as a
slot of its box's graph — is not available: a slot's members are fixed by its
def, and what plays a note is whatever its events name. So the clip is the
strip over a **bus** (`clausters_core::mixer::voices_graph`), the bus is the
client's to allocate, and a voice is told it as its `out`: the control an
event's `out` key already sets, which makes it a convention a def either
follows or not rather than a new one. A def without the control sounds where
it was written to, and that limit is stated rather than worked around —
capturing whatever a group writes on the hardware's buses would catch
everything else that writes them.

**One lane, and a group before the multitrack.** The lane stays one per
multitrack, since what makes a voice its box's is the bus and not where it is
made. But a voice has to have written its bus by the block the clip reads it,
so the notes are made in a group before the multitrack's graph, inside the
transport's group and outside the tracks': they read the transport, and a stop
releases them rather than freezing them mid-release. The release itself is
then heard only as far as the stopped track lets it, which is what a take
does.

**Which boxes play notes is said by the notes.** The document names a source
and never what is behind it, so the node plan cannot know a box is over a
sequence. Rather than a new argument through every door — two clients and the
host — the playback plans a clip for each box its placed notes name, against
the multitrack the last sync was of (`nodes::plan_voiced`). Nothing crossed a
binding.

**Mute moved to the strip.** A silenced box's notes used to be left out of the
lane; they are now placed and sound into a muted strip, because that is what
a mute is for a take — a control of the fader, lagged, automatable, and one a
note that began under it comes back from. The cost is that a muted track
still runs its voices. A MIDI message and a command are still left out of a
silenced box: no strip reaches them.

**Two things in the server made the built-in `default` usable for it.** It
wrote buses 0 and 1 as constants, so an event's `out` did nothing on it; it
has an `out` control now. And its second channel is `out + 1`, a bus index a
UGen computes, which the bus analysis read as *dynamic* — a barrier the
auto-sort cannot cross, in every graph that holds such a voice. The analysis
now evaluates arithmetic over controls and constants (`osc::graph`), so the
index is static and a `/node_set` on the control re-sorts as for a bare one.
The barrier had already cost something on the way: a clip's reader that
computed `out + chan`, as the audio editor's does, put a track's meters before
the strip they read, so the fix for a stereo take's two sides hands the reader
both channels of its bus as two controls instead. That holds whichever way
the analysis reads arithmetic, and it stayed.

## A grid is a length in beats, and a beat read back is settled on its line

*(2026-10-08.)* Quantize did nothing in the notes editor or the multitrack: a
view snapped to a `snap` in its own units, and no application set one. The
grid that was missing is a musical one, and a musical grid is a length in
**beats** — a sixteenth is a quarter of a beat wherever it falls — while the
views measure samples, and a tempo map may change along the axis. So the
grid travels as beats (the axis' `grid` prop) and the host resolves it
through the axis' own tempo map (`TempoMap::snap_secs`): a line is a beat
wherever the tempo puts it, which is where the beat ruler draws it. A grid in
samples would have been right only where the tempo never moves.

**What a quantize leaves is the document's**, as an edited take is the audio
editor's: the notes are written back into the sequence, and the file saved
from it holds them quantized. A raw performance stays raw until somebody
quantizes it; a quantized one stays quantized. Only the grid is the window's
— a presentation choice, like its zoom, which changes nothing by being set.

**Snapping to the grid is a switch, and the quantize does not read it.** A
grid used for every placement takes the hand's freedom away: a note could no
longer be put a little ahead of the beat. So the axis carries `grid_snap`
beside `grid`, on by default — a hand lands on the grid while it is on, where
it puts things while it is off — and the quantize moves onto the grid either
way, because moving onto the grid is all a quantize is. The switch is a verb
of the window (`snap`, G, the View menu) and a property of both clients'
editors.

**A beat read back through seconds is settled on the line it was meant
for.** The view places in samples, so a note quantized to beat 3 comes back
as `3.0000000000000004` wherever the map is not trivial, and a document
would store that. `TempoMap::settle_beat` returns a beat within a billionth
of a beat of a line of a 960-per-beat grid as that line exactly, and any
other beat as it is: 960 is divided by every common division of a beat
(halves, thirds, quarters, fifths, sixths, eighths and their powers), so a
quantized note lands exactly and a note played off the grid is not moved by
the trip. The notes axis applies it to every beat it reads back.

## A voice is notes and their joins, realized by each destination

*Decided with the user, 2026-10-10.* sclang has `Pmono` and `PmonoArtic`, and
sc3 merged them into one pattern with a flag; its events carried `server` and
`node_id`, the state of a sounding server, and no curves. `pvoice` keeps the
merge (the flag is `artic`) and changes what the events are: **every event is
a note**, and what the voice adds is the **articulation** between consecutive
notes, which the note's `legato` says. The framing is the user's, from the
spoken voice: an articulation is how distinct notes relate (legato, a tie, a
slur), an inflection a variation inside one note (its `expression`). A change
of note is an articulation in every abstraction, MPE included, where a note
keeps its note-on and only its bend moves.

So the datum is one and each destination realizes it. On the server the core's
event render reads two keys a voice writes as it plays: a note that `continues`
a node sets that node instead of starting a synth, and a `held` note schedules
no release -- one synth across the joined notes, as `Pmono` has it, by one rule
for both clients (`clausters_core::event::render::synth`). On MIDI the joined
notes simply overlap. Rendered into an `EventSequence`, the recorder hands each
note an index as its node, so the next note's `continues` names it; those joins
go to `clausters_core::event::notation::articulation`, which says which are
ties (one pitch held) and which runs are slurs, and the render writes both: the
`tie` key and a slur spanner over event ids in the sequence's notation. The
sequence keeps the `legato` too -- both shapes, as the user asked.

A voice reads **one note ahead**, to know whether the note it is about to
yield goes on into the next. The cost is that the next note's keys are read
with the incoming value of this one, so a `pkey` inside a voice sees the event
before; no pattern of the library depends on that, and knowing the join at the
moment a note starts is what lets the server never re-attack a joined note.

## A take's tail is the caller's, and there is none by default

*Decided with the user, 2026-10-10.* The `render` verb grew a `tail` on
2026-09-27 -- seconds a bounce goes on past its last event, so the last note's
release is rendered -- with a default of one second, and `Session.render` was
left without one, on the reading that a session's score is "written" and says
its own length. That reading did not hold: a session's score is what was played
into it, a pattern's gates included, and the same cut release reached the
patterns example through the method the verb had been fixed for. So both have
the same `tail`, closed by one rule (`close_score` on the offline interface of
each client), and **its default is 0**: how long a take should ring is not
something the client can know, so an unstated length is clearer than a second
nobody asked for. A take ends on its last event unless the caller says
otherwise; the examples that want a release heard ask for one.

## Audio preview owns the insert id and dry bus (2026-10-16)

A GraphDef slot is a group whose member ids are private. `EnvGen` with done
action 1 pauses its own synth, and `/node_run` on the containing group cannot
resume that member. The audio playback therefore allocates each file's dry
bus and inserts an ordinary caller-owned synth inside the auto-sorted play
group. Reader and pass ports name that same bus. This keeps the ramp and
pause sample-driven in one node, without client timers or new commands.

The processed picture is a scratch buffer read through `preview_buffer` on
the existing signal body. It is superposed after the original picture and
never becomes an edit or copy source. The existing plural-buffer loader and
summary walk serve both sources. A resumed effect retains its frozen state;
transport-addressed gain immediately follows the current position. A chain
reorder rebuilds the affected suffix with its saved ports and fresh state,
since an auto-sorted group refuses manual node moves.

*2026-10-23:* the gain envelope's preview was removed with the takes'
move onto files, and its curve is heard once it is applied
(`crates/clausters-apps/PLAN.md`, the turn to disk's eighth decision). What
is above about the insert's id and bus holds for an effect heard live, if
one is added.

## An application's dialogs are one mechanism, and a new file is the client's to open (2026-10-16)

The score editor's dialogs -- forms on the pages of a stack the window holds
from the start, opened by correcting the stack's index -- were its own module,
and the audio editor needed the same thing for two forms: a new file's shape
and a save-as path. Rather than a second copy, the mechanism moved to
`clausters_apps::forms`, generic over what a form is (`forms::Spec`: its
prefix, page, title and fields), and both editors describe their forms to it.

The new-file form does not open the file itself. A window's holder decides
what a window is (a client opens an editor, a standalone host its own), so its
`OK` answers the shape as the outcome's `newFile` and the client opens
`edit(SoundFile.empty(...))` beside the window. A file not written yet is a
save target with no path: the window's save opens the save-as form, and a
script's `save()` refuses without a path rather than guessing one.

## A take bounds its axis, and an audio editor plays on its own transport (2026-10-17)

Two behaviors of the audio editor had drifted from what it was. Since the
gain envelope joined the take's pane, the curve made the pane's navigation
group look like an authoring surface (`roll_grid` reads any member with
authored content), so a two-second take zoomed out to sixteen seconds -- the
empty grid of a roll. A view of stored samples now **bounds** its group
(`OnAxis::bounds_axis`): the group navigates the take's extent, with no
headroom and no grid, and the other members are read on that axis without
lengthening it. Headroom stays where a group has no take.

And the audio editors of a server shared one playback, so playing one window
moved the play cursor and the meter of every other. A window is one object,
and a shared cursor is for views of one object, so each editor now has a
playback of its own (one transport, one meter); the crate's playback still
holds several files with one sounding, for an editor that holds several.

Both rules, and the others the applications keep, are written in
`docs/app-behavior.md`, which a change to an application is checked against.

## The files an audio editor adds walk orders of their own (2026-10-17)

An audio editor holds several files, and the user decided that undo is per
file. That meets the editing context's rule that every window in it walks one
order: two windows over one take share an undo, and a multitrack and the take
of one of its regions do too. Both hold. The file an editor was opened over
is a structure of the context like any other, and its edits stay in the
context's order, which every window over that take shares. A file the editor
adds, opened beside it or made empty, is reachable from that window alone,
so it walks a `History` of its own, held by the context and swapped in while
that file is in focus. No new history type was made; the existing one is
instantiated per added file.

A step therefore names whose order it walks. A window's undo and its menu
state name the window's member (`step` and `state` with `member`), and the
member's file in focus picks the order; a step that names no member walks the
context's. A turn of a member selects its own, so a key pressed in the window
needs nothing more. Closing the opened file while others stay forgets its
structure in the context's order (unless another window still holds it), so
an undo there never reaches a file that is gone.

The rest of what a file keeps (parts, gain curve, cursor, selection, layers,
loop switch, unsaved changes) is the application's state for that file, parked
in Rust while another is in focus. All files share the editor's one playback
and transport: a running pass continues while another file is viewed;
stopped, a switch locates it to the incoming file's cursor.

## Audio component ids belong to the window binding

*2026-10-18.* A file switch changes which contents an audio window observes.
Naming its components by the focused structure gave the next picture new ids,
while focus corrections had already been calculated against the previous
picture. Sending those corrections after publication addressed widgets that
were gone. A real-window run exposed this as `no such widget`; a fake host
without the real allocator could not exercise that lifetime.

Audio components now use the drawing instance's allocator identity, through
`Application`'s internal view-id helper. Separate windows retain separate
component names even when they observe one structure. Switching files keeps
the window's component ids and rebinds their data; the editing context's
version rejects a gesture from the old file. The host's existing named-id
retention and renderer reconciliation remain the mechanisms underneath.
Preview corrections follow publication, and only composed lanes receive
cursor corrections. History and sample resources remain attached to the
file's stable object identity, independent of the display buffer.

## Application resource ids include a connection incarnation

*2026-10-18.* Buffer numbers are local to a running endpoint. A restarted
server can allocate the same number again, and two connections can name
unrelated buffers with the same number. An audio editor's source retention
and clipboard replacement must not free a replacement server's buffer.

The application resource service uses a connection identity, an allocator
incarnation and the local buffer id. Both language adapters assign a random
connection token and advance the incarnation when attachment replaces the
allocator. An old editor refuses work; invalidation forgets its leases without
sending frees. The clipboard's existing per-server slot carries that same
incarnation when replacing its copied buffer. Connections are treated as
separate until a later exchange contract explicitly establishes compatibility.
This requires no new server command or ABI symbol and keeps server detection
at the connection adapter. Publication, cancellation and stale in-flight
completions remain responsibilities of the operation coordinator.

The existing history engine continues to walk whole entries. Application
object membership selects an order, and endpoint-aware byte accounting is an
input to trimming; neither an editor class nor a window decides which subset
of an entry can be undone. Source ownership lives in the resource service,
with independent roots, rather than treating a history release as permission
to free a buffer immediately.

## Audio clipboard publications are leases, not undo sources

An audio editor copies a potentially large range on the server. Moving its samples through the client and host would add two bulk transfers; naming only a buffer number instead lets another server or a recycled id be mistaken for the copy. A source window also cannot own a copy that must survive its close. The application exchange therefore owns an immutable publication scoped by connection incarnation and publication identity, using the common resource service for clipboard and pending-reader roots. A paste copies into destination-owned samples, whose ordinary history may spill to disk or OPFS. Replacing the publication releases it after its readers finish and cannot release a destination's undo sources.

Copy is published after server completion; Cut records removal only after publication and a matching original-target checkpoint. The checkpoint crosses language adapters as opaque serialized text, avoiding numeric normalization of its contents by JavaScript. General operation scheduling and failure rollback remain separate work. Server-backed copies across connections are explicitly refused rather than silently transferring samples or trusting matching endpoint addresses. Channel conversions and resampling likewise require explicit domain operations; insertion preserves a source rate through the existing parts ratio, while mixing requires matching rates. *(Superseded in part 2026-10-28: a paste of another width now goes by a rule decided with the user, duplicating a mono block, mixing into a mono take and laying channels in one by one otherwise, through `clausters_core::channel_matrix`. Resampling is still never implicit.)*

The host mirrors typed publication as text and remembers the observed platform value. An unchanged mirror preserves the typed payload; newly observed external text is always text, including text that resembles clipboard JSON. Remote publication ownership is reconciled on the next application clipboard action rather than by background polling. This can retain one unused publication per endpoint until the next action or endpoint disposal; a pending reader can additionally retain a replaced publication until completion. A browser host uses the same policy without acquiring platform clipboard access.

## An audio action is a run of effects and awaits in the shared core (2026-10-21)

*Revised 2026-10-25 ("A member's run is driven in its application's
process, and JSON is spoken only at a client's door"): the run, its order
and its unwinding stand; the JSON vocabulary its two clients needed does
not, since no client runs it any more.*

Each client used to decide what an audio editor's action led to: which
playback verb follows a focus change, when the meter is read, when a copy is
published and its cut committed, when freed takes are released. The two
clients drifted in exactly those places, and a fix in one (the meter read
before the playback was told the file viewed) had to be made twice.

The order now lives in one coordinator per audio member
(`clausters_apps::editing::audio_session`), which owns that member's
playback. A trait the clients implement would have put the order back in
each binding's callback plumbing, and Python answers synchronously while a
page cannot: so an action is a **run** that answers its effects in order and
stops on an **await** for what only the client can do (steps on the server,
whether the transport rolls, a file read, an operation's batch, a history
step, a clipboard publication), resumed with the result. A failed await
unwinds through the cleanups and handlers the run passed, as an exception
would, and the client raises what failed, so a client never decides what a
failure means.

The order is Python's, because Python was the reference. Two consequences
for the web client follow from keeping it. The effects a run decided before
its first await are carried out at once, so what the page owes synchronously
(whether the take changed, a refusal's reason, the layers it shows) is there
when a setter returns; the rest waits in the page's queued work. And work a
run's await sets off (the restore a history step needs, the store a release
asks for) is carried out inside that await, before the run resumes, where
queueing it behind the run would answer the window before the take it reads
was stitched. A trace both client suites replay against
`audio-trace-vectors.json` is what keeps the two from drifting again.

## An application's takes live on disk, and the server holds audio functions (2026-10-22)

*Decided 2026-10-22 by the user, after successive pastes of a long selection
in the audio editor failed natively with `no reply to /buffer_stitch`.* The
editor read each file whole into a pool buffer, copied a selection in the
server's memory, and, with a shared segment, copied every new join whole into
a region a peer maps. An edit cost a pass over the take and memory grew with
the history, so a take of hours was out of reach whatever each step was made
to cost.

**Pool buffers are finite and short**: a sampler's samples, an oscillator's
table, a stroke being drawn. **An application's takes are files**, of any
length their format admits (W64, RF64), and are never read whole. The same
segment logic reads either: a sampler reads segments of pool buffers with
random access, an editor and a multitrack read segments of files as a stream.

**A session holds metadata**: an edit of the audio editor is a new segment
list over files that never change, as an edit of the multitrack already is, so
cut, copy, paste, undo and redo move no samples at any length. **The RT server
plays and previews**, reading files ahead of the playhead; **the NRT server
processes**, reading spans from disk and writing a new file that the list then
names, temporary until a save keeps it. Renders share no state, so several
NRT instances run at once, and the RT server plays beside them. **Drawing reads the files' peaks**,
composed by the host, so it needs no server.

**The server is the module of audio functions, not of editing logic**: no
command exists for one application's verb. That retires the commands added
for the editor's verbs (`/buffer_gain`, `/buffer_reverse`, `/buffer_mix`,
`/buffer_render`); an edit's processing is a def rendered offline.

The model, its decisions and its milestones are in
`crates/clausters-apps/PLAN.md`, "The turn: an application's takes live on
disk"; the server's half is the server plan's `S21`-`S23`, the document's
`O34`, the host's `G45`.

## A render's progress is a window prop, not a status line (2026-10-23)

An edit that renders runs its offline render in the client, and a gain over
an hour takes long enough that the window has to say it is working. The
status bar is where a window says things, but by design nothing on the wire
writes a line of it: the bar is the host's account of what the hand did and
what came back, and making an owner re-send that would carry a fact back to
the machine that produced it.

**A render's progress is the one fact the host does not have**, so it crosses
as the window's `progress` prop -- a fraction in `0..1`, with
`progress_label` -- which the host draws at the right end of the status bar
as a name and a bar, and a negative value takes away (OSC has no null). It is
state rather than history: a second render replaces it, and the log keeps
only what the host did. The client sets it from the progress the render
reports, at most ten times a second, and takes it away when the render ends
either way.

## A render is carried out beside an editor's work, not in its queue (2026-10-23)

An audio edit that makes samples is an offline render, and a gain over an
hour is a long one. Both clients used to carry a run's awaits out in order:
the Python client on the window's event loop, which a render blocked, and
the page in the editor's queue of work, which a render held. So a second
file's edit waited for the first file's render, though nothing in it
depended on that render: the operations already lock per file, and the
coordinator resumes a run by its number.

**A run that stops on a render hands the render out and lets the editor go
on.** The Python client carries it out on a thread of its own when the run
is on the window's loop, and posts the rest of the run back to the loop, in
a turn, when it lands; a script's call still waits for its end. The page
carries it out beside the queue and queues the rest of the run when it
lands, and a page's promise still resolves at the end. A batch's endpoint
lock covers each request that waits on a `/done`, never a render, since two
waits for one command's `/done` on one server cannot be told apart but two
renders share nothing.

The trade-off is in what waits. An edit of the file a render is out on is
refused while it is out ("the file has pending work", which the window's
actions already read) rather than queued behind it; an edit of any other
file runs at once, and two renders land in either order, each committed to
the file it was made on. A history step is ordered by when entries commit,
so an entry of a render that lands later is later in the history. Waiting
for every member's queued work, which a history step does before it redraws,
does not wait for a render out: that render commits after the step, and
waiting on it from inside the queue its rest is queued on would never end.
A page or a test that wants everything landed waits from outside the queue
(`AudioDomain.settled`).

## An editing application holds its engine, and the server holds no application's lists (2026-10-23)

The first turn (above, "An application's takes live on disk") put the audio
editor's take on disk as a segment list, and put the list in the server: a
`/buffer_stitch` whose parts name files, read by a stream reader inside the
server, with the editor's peaks measured by server renders. Measured with a
pattern of copies and pastes that reaches hours (ten seconds pasted five
times, that minute nine times, those ten minutes six times, and on), it
fails in three ways. A paste flattens the clipboard into the list, so the
list grows with the product of the pastes (6, 51, 351, 2,451, 12,251, 49,001
parts) rather than with the work. Every edit rebuilds the list and sends it
whole, to the server and to the window, so an edit costs the take's size
(784 ms at 163 hours, in the editor alone). And at about 2,500 parts the
stitch outgrows a packet: the server never receives it, the editor waits for
its `/done`, and the application hangs.

The cause is not a slow function but where the model lives. The server is a
real-time sound server, close to a plugin host; an editing application
handles its information otherwise. The field's editors keep the edit
structure, the disk reading and the peaks in the application, sharing
immutable blocks between the take, the clipboard and the history so a paste
costs what it edits, and a thread of the application reads ahead of the
playhead into one ring per channel; the real-time engine receives only
samples already read, and an edit sends it nothing.

**So an editing application holds its engine.** The audio editor and the
multitrack are each one program, natively and in a page: the host, the
application's domain and the engine, embedded and spoken to by direct calls
(built commands through the engine's FIFO, the transport read from
atomics), with no OSC and no external channel between them. The
application owns the take's structure, its reading into rings an engine node
consumes, its peaks and a multitrack's session file (the audio editor's
was withdrawn on 2026-10-25: a save consolidates its take, so it keeps an
edit in progress for nothing), all through the core's functions,
identical bit for bit in both places, and written once for both
applications. A client drives the application by its handler's verbs, and
an effect a client sends is added to the application's own list of effects,
which loads it into its engine.

**The scope is these applications.** The clients keep their live, offline
and embedded servers, the server and host to client design stays, and so
does the shared memory between them. The server's transports keep their
role: patterns and sound generated in real time, sample-synchronous playback
of files, and OSC and MIDI messages. What the server held for an
application's domain is audited and removed (`crates/clausters-server/PLAN.md`, `S23`).

Three answers of the user the same day set its edges. The application
speaks to its engine with built `Cmd`s, never OSC, building them with the
server's own def compilation taken as a library. The audio editor edits
sound files only: a `Buffer` lives in another server, which the
application's engine does not reach. And an editing application in a page
requires cross-origin isolation: the rule that the page's engine never uses
shared memory, so as to embed in pages whose headers nobody controls (above,
"The browser engine is one wasm instance inside the AudioWorklet"), did not
hold up, and does not bind these applications.

The trade-off: the application now carries an engine and a reader thread of
its own, and a page needs shared memory between its worklet and its worker
(a `SharedArrayBuffer`, wasm threads and origin isolation). In exchange an
edit costs the edit, nothing crosses a process boundary per edit or per
block, and the server's surface stops growing with the applications' needs.
The plan is `crates/clausters-apps/PLAN.md`, "The second turn: an editing
application holds its engine".

## A take is a tree of shared parts, held by reference (2026-10-24)

Context: a copy and paste pattern that reaches hours (ten seconds copied and
pasted five times, the minute that makes copied whole and pasted nine
times, and so on) made the audio editor's take a flat list of 49,001 parts
at 163 hours. Each paste wrote the clipboard's parts into a new list as long
as the take, the clipboard and every history entry carried the list as
JSON, and the window was sent it whole: the work of an edit followed the
product of the pastes before it, and the last ones took most of a second.
The second turn left the structure to this milestone: bounded blocks of
samples shared between the take, the clipboard and the history, or regions
over files, or a tree of either.

Decision: **a tree of the parts the list already has** (regions over files),
balanced by height and with immutable nodes shared by every take that holds
them (`clausters_document::parts::Tree`). A span, an insertion and a
removal make the nodes on the paths to their seams and share the rest, with
the flat list's arithmetic and its merge rule at the seams, checked against
it. Whatever holds a take holds a reference: the clipboard document names a
copy the application keeps (`HeldCopy`), a history entry names the take
before and after by a number held beside the files it reads and let go of
with them, and the window is handed the tree in the same process
(`{"shared": n}`), whose picture the host composes from the nodes a column
holds whole, each node's summary kept by its identity. Measured in release
over the pattern carried to 190 hours and 58,800 parts
(`crates/clausters-gui/tests/shared_blocks.rs`): a paste answers in 0.01 ms from the
first to the last, an undo or a redo in microseconds, the window is told
about 830 bytes, a whole picture is drawn in a few milliseconds reading a
few thousand parts, and memory grows under a megabyte.

Rejected: bounded blocks of samples. A copy of a span would render its two
partial blocks into new ones, so a paste writes samples, and an ordered
array of blocks still costs a paste the take's length, which only a tree
avoids. The parts already name immutable files, so the tree shares them
with nothing written. Also rejected: keeping the JSON forms and making
them faster, since a list written out per edit costs the list however fast
it is written.

## A member's run is driven in its application's process, and JSON is spoken only at a client's door (2026-10-25)

*Decided with the user, 2026-10-25, reviewing the audio editor's code
before the multitrack is put on its modules.* The coordinator's runs were
written in JSON (2026-10-21) because two client languages carried them out,
one synchronously and one not. Since the GUI host's application holds the
editor's engine (`X30`), its only caller is Rust in the same process, and the
JSON had become four doors deep for one gesture: the application wrote a
request as text for the context, which wrote one for the coordinator, which
wrote one for the context again and for the editor, each answer parsed back,
and most awaits were answered at once by that same caller.

**The run stays and its driver moves beside it.** A member's action is still
a run of ordered effects that stops on an await and unwinds a failure through
the cleanups it passed; what changed is who answers. The loop that answers the
awaits is the context's (`Editing::start`, `land`, `query`), over what only
the caller does (`editing::Caller`: a file's header, the files on disk, steps
carried out on its turn, files deleted, renders called off), and the one await
handed back is a render, which the application runs on a thread of its own and
lands. Awaits and answers are typed; the playback's JSON door went.

**And the vocabulary went after it (2026-10-29).** The first pass left the
coordinator calling the context and the editor by their doors over values
(`{"verb": "binding"}`, `{"verb": "operation", "action": "pendingRepairs"}`),
which is the same vocabulary without the text. The coordinator now calls them
by methods: an action enters typed (`AudioAction`, read once from the door's
JSON by `Editing::start`), an outcome is the editor's `Outcome` with typed
fields (the file a window named, the files to open, a new file's shape, the
disk to look at, a cut's second half), an operation's answer is the
protocol's (`application::runtime::Protocol`) and the member's end of it
(`Operated`), and what the application carries out of a run (files deleted,
renders called off, a clipboard lease let go, by its number now) is typed
apart from the window's effects. The JSON doors read a request into those
calls and write the answer out, so a client's answer is the same.

**What stays JSON, and why.** The window's effects, since the host draws them
as it draws any client's GuiDef and props; a run's result and a client's
answers, since a client's door (`/gui_app_call`) reads them; the facts an
editor is built from; and every door a client or the C ABI binds. A typed form
of those is a change of the protocol, not of this design.

**What it keeps out of the multitrack's way.** The engine reads a take per
reader, not per audio file (`HeldTake::reader`), the application that holds
the engine drives any member (`app_engine::app::App`), and what is handed on
by reference is kept per editing context (`audio::shared::Shared`) rather than
by the process, so a member that leaves the context lets go of what only it
named.

## A WAV past 4 GB is closed as an RF64 under its own name (2026-10-28)

**Context.** A save writes the take into the file it was opened from, and
a `.wav` the hand pasted to hours outgrew what a RIFF's 32-bit sizes
count. The writer refused at the write that crossed 4 GB, after writing
four of them, and the length is only known at the close for every writer
that streams: a render, a recording, a page's writer in pieces.

**Decision.** Every WAV written keeps an RF64's room: a `JUNK` chunk the
size of a `ds64` before its format, which makes its header exactly as long
as an RF64's. At the close, a body past 32 bits rewrites that header in
place as an RF64 (`RIFF` to `RF64`, `JUNK` to `ds64` with the real sizes)
and keeps the name it was given. This is how EBU Tech 3306 says to write a
WAV that may grow, and what the field's recorders do.

**Consequences.** One rule in `pcm::header`, so the native writer and a
page's writer, which both ask for the header at length zero and again at
the close, agree without either knowing the length beforehand. A WAV
written here is 36 bytes longer than the canonical 44-byte one; every
reader skips a `JUNK` chunk, and the server's readers already took an
RF64 named `.wav`.

## A multitrack's tracks are read by its application, from their files (2026-10-29)

*Decided with the user, putting the multitrack on the audio editor's
modules.* A client's multitrack read every take whole into a server's
buffer and played each region through a clip of readers over it, which an
hour-long take, or a session of four, makes a load of gigabytes before the
first sound.

**A track is one take the application reads.** The multitrack application
(the GUI host's, as the audio editor is) reads each track's regions from
their files into a fed stream per channel of the track, as it reads an audio
file's parts, and the engine plays that stream with one reader per channel
(`mt.feed`). What a region does to what it reads -- its place, its window,
its playrate, a loop around its source, its edges and its gain curve -- is
the core's arithmetic, once (`clausters_core::regions`), heard against the
old clip reader to 1e-4. So an edit of a track's regions hands the engine a
new take and makes no node, a session opens in the time its headers take,
and the process does not grow with its takes.

**What this changed, on purpose.** A region plays by the implicit channel
map, a mono source at unity on every channel of its track, and a track is as
wide as its widest region; a mono take on a track that was declared stereo
is a mono track now, panned into the master as a track is. A region's edges
keep the multitrack's own rule (each capped at the span), not the audio
editor's (half the part), so a region sounds as it did. A region's gain
curve is read per sample, where a clip's was a control bus a block.

**Where the peaks are.** A session's takes keep their peaks in the session's
own `peaks/` folder, measured once by a render when they are missing, never
beside the user's files, which a session neither writes nor copies.

**The notes and the roll go with it.** An event lane's feeder is a struct
the server and the application's engine both hold, so a box of notes plays
on the application's engine, and its roll opens in the same application, one
editing context, so an edit in either is one history.

**`--session` opens that application.** The host's own multitrack path, its
in-process server and its player are no longer started for a session.

**The roll plays on a transport of its own.** A roll opened from a box plays
its sequence on an event lane of a transport the window takes from the
engine, as a client's notes editor does, so its pass never moves the
multitrack's.

## A script's multitrack is the window's while it is open (2026-10-29)

*Decided in the plan the user approved for moving the multitrack onto the
application.* With the multitrack in the GUI host's application, the
`Multitrack` a script built or read from a session is no longer what any
editor edits: the window holds one of its own. Two copies that each take
edits are two multitracks that drift.

**While the window is open the script's multitrack is the window's.** Its
objects ask the application through the door (`handle`): a read answers what
the window holds now, and a change made through an object is an edit of the
window's, recorded in its history under the label the object gives it, so the
window's Ctrl+Z takes back what the script did. `multitrack.history` is then
the window's history; a block (`with multitrack.history("tidy")`) applies each
change as it is made and records them at its end as one entry. **When the
window goes it hands the multitrack back**: the close carries the multitrack
as the window left it (`/gui_closed id json`), and the script's multitrack
holds that from then on, its own again.

**A page cannot read through a promise.** A call to the host is asynchronous
in the web client and the multitrack's objects answer at once, so a page's
multitrack keeps a copy: a change is made in it and forwarded to the window in
order, and the copy is brought to the window's after each of the page's own
changes and steps and when the window closes. The surface is the same in both
clients; this one difference is named in the web client's prose.

**What a hand does is told, not read through a promise** (decided by the
user 2026-10-30). Of the two ways to bring a hand's change to a page's
reads -- the application telling the page each change, or every read a
promise -- the first keeps the multitrack's surface as it is in both clients
and asks almost nothing of either: the host tells the window's opener
`/gui_app_changed window version` after a gesture, a verb or a step in the
window (or in a roll opened from it) moves the context, and the page brings
its copy to the window's as it does after its own changes. The script reads
the window at every read and needs nothing of it. The opener's own calls are
not told, since it already follows them.

## The server keeps no command an application's editing made (2026-10-29)

*Decided by the user 2026-10-29, auditing what is left of `S23`.* "The
server is the module of audio functions, not of editing logic" (2026-10-22)
is read to the letter: every command the server grew for an editing
application goes, whoever else came to use it. So `/buffer_stitch` goes
whole, joins over buffers as well as lists over files, with `/buffer_parts`;
`/buffer_peaks` goes with the summary the server kept on every buffer, and
a host draws a server buffer from samples it reads, summarized with the
core's measure; `PeaksOut` and `/transport_cue` go. What a live client plays
and records with stays: a buffer's reads and writes, `/buffer_touch` (a
peer's write announced) and `/buffer_stream` (a recording's progress), and
the transport's play, stop, locate, loop, end mark, fade and follow. The
applications measure, read and join their takes themselves, with the core's
functions (`crates/clausters-server/PLAN.md`, `S23`).

## A live parameter's undo writes the value it had (2026-11-02)

*Decided with the user, opening `X26` (`crates/clausters-apps/PLAN.md`).* A
panel's control bound to a node's control is not the only writer of that
parameter: a controller through the server's MIDI bindings, another client,
a plugin's own window, a bus and the script's `/gui_set` all reach it, and the
host sees them only as a read-back that differs from what it showed. So an
undo of the hand's last move may find the parameter somewhere the entry never
left it.

Three answers were weighed. **Dropping the parameter's entries** when another
writer moves it is the rule the host's text field already has (`/gui_set
value` starts its history over), and it was the first proposal; it reads a
parameter as a document the script replaces whole, which it is not, and a
parameter a bus drives would never have an undo. **Refusing the undo** while
the value differs keeps an entry nobody can walk past. **Writing the stored
value** is what audio software does with a parameter's undo, and what was
chosen: an undo writes the entry's first value and a redo its second, whatever
the parameter holds; a writer that keeps driving it takes it back on its next
write, which costs nothing, and a controller left where it was may disagree
with the value until it moves, as it does elsewhere.

The rule has a general half and a half that applies only here. **For every
application**, what is played is not an edit: live input makes no entry, a
hand's gesture is one, and a recording pass is one entry of the application
that records it (`docs/app-behavior.md`). **Writing the stored value matters
only where a value has several writers**, which is a live parameter. A
document has one owner and nothing changes it outside its history, so its
undo restores exactly what was there, which is the same thing seen from the
side where nobody else writes. A multitrack's knob over an automated port is
that case already: the hand holds the port while it holds the knob, and
nothing is written or recorded.

## An id from outside is taken in, never trusted (2026-11-10)

A sequence's events and curves, and a multitrack's tracks, take lanes,
regions, markers and curves, each share one id space, and the views name what
they draw by those ids. Two things under one id are drawn as one and edited
into each other: a roll that met a pedal curve under the id of a note's bend
wrote each one's points into both on every edit, until the host asked for a
384 MiB vertex buffer. The ids were minted by a counter, but some code
computed them by hand and every door that brings ids in (a file, a client's
list, a whole sequence restored, a pasted block) trusted them.

The rule is now **generation by construction and reconciliation at the
door**, not a check of every id on every edit. What the document makes it
mints. What comes in is taken in once, where it comes in:

- **A sequence** repairs whatever it is given whole (`events::ids`): a file
  read, a MIDI file, a client's JSON, a `Restore`, a derived page, a
  `SetEvents` list. **The events choose first**, because an event is named by
  other events (a join, a slur's ends, a selection) and a curve only by the
  views that draw it; a curve that loses its id is renumbered and nothing
  else notices. One event or one curve coming in is checked alone. A curve
  naming an id the sequence holds as another kind of thing is refused, since
  the caller meant something this sequence has as something else. What a
  repair renumbered is in the edit's answer and in the log.
- **A multitrack** is repaired when it is read (`multitrack::ids`), but an
  intent that names ids (a track list, a take lane stated whole, a split's
  halves, a join, a new marker) is **refused** when one already names
  something else. Its ids are the caller's, minted through the handle, and an
  intent is absolute: renaming what it brought would apply something other
  than what was sent, and the client would hold objects under ids that no
  longer name them. The multitrack keeps no counter, so its next id is one
  past everything it names, markers included (they were left out of one of
  the two minting functions).
- **A symbolic score** repairs repeated item ids before every operation and
  at the end of reading a document. The reader itself numbers the parts of a
  note split across a barline under the note's one id until it joins them, so
  inside the reader a repeated id is the item's and not a clash.
- **The general tree** keeps refusing a document whose ids name two different
  nodes, as before; a members list that would make one is now refused too,
  since it would leave a document no reader opens.
- **The host mints no document id.** A note it copies (the tail of a cut)
  goes with none, for its owner to name.
- **A transformation keeps identity where the thing is the same thing.**
  Writing a sequence as notation renders a page numbered from one; its notes
  that are the performance's notes take back their ids, its curves the ids of
  the curves they are, and only what the page adds is minted, with every
  reference renamed at once. A cut made in a window is read as the region it
  was cut from, not as a new box.

## A file says which application it belongs to, and the format is the file's (2026-11-10)

**Context.** The host's flags named its applications by history: `--session`
opened the multitrack because it was once the only application with a
session. And the roll converted a sequence between MIDI and notation in
place, as though one sequence were every format at once, so what one format
holds and the other does not (a performance's joins first) was lost by a
switch of mode.

**Decision** (the user's).

- **The flags name the applications**: `--multitrack`, `--score`, `--audio`
  and `--roll`. `--session` is gone, with no alias: the project is before
  1.0 and no client launches the host with it.
- **A session file stays `.json`, and says whose it is.** Every
  application's file is JSON, so the extension cannot tell them apart; the
  file's `metadata` names the application (`{"application": "multitrack"}`).
  The three writers (the crate, both clients) write it, every reader defaults
  a file without it to the multitrack and refuses one that names another. It
  is an added field, so the session format does not move.
- **The format is the file's.** A sequence is opened *as* a file of one
  format -- MIDI 1.0, MIDI 2.0, MPE, MEI or MusicXML -- the roll offers only
  the editing mode that format supports, and a change of format is an
  explicit conversion of the file into another. The alternative, a table of
  what each format holds and what a switch carries or keeps aside, was not
  taken: it keeps one object pretending to be several, which is the
  confusion the decision removes. The design is in
  `crates/clausters-apps/PLAN.md`, Future directions, "What a sequence is,
  as a thing that is edited, and what a change of representation keeps".

  *Built 2026-11-11 (`X44`).* The roll's file reuses the score editor's: the
  file runtime (prepare a revision, store it, confirm it, change the window's
  document by the closing rule) became `clausters_apps::file`, one module for
  every window whose document is one file, each editor saying only how its
  document is written and read (`FileWindow`). A store step now carries bytes,
  since a MIDI file is not text; its JSON form keeps a text document as text
  and writes any other in hex, as a render's score is. The applications crate
  links the MIDI file reader and writer (pure Rust) so the roll's file and the
  score's export are written by one function, and the engraver stays a port
  the host hands in (`Editing::read_scores_with`), as its outlines are.
