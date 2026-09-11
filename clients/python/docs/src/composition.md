# The document: what a composition is, and who edits it

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is a piece placed in time,
with tracks, takes and curves that are **authored, durable and undoable** — state
a picture cannot hold, because a picture is drawn and drawing frees.

That state lives in the **document** (`crates/clausters-document`), and this
chapter is about it: the model a multitrack editor edits, the presentation beside
it, and how an edit is applied, inverted and saved. It is one crate, bound by
every client and by the `standalone` host, so what an edit *means* is defined
once rather than re-derived per language.

**The data is what is fundamental.** Samples, notes, events, curves — they are
edited and drawn with no arrangement anywhere near them: `edit(x)` opens a
buffer, a timeline or a curve on its own. So the pictures are independent of the
model too — **a clip is a view configured by what it holds**, and the edits it
admits (move, trim, split, join) come from the structure inside it, in the unit
that structure measures. A clip over samples and a clip over a timeline of notes
take the same actions; only the arithmetic differs.

`clausters.form` is a **relegated** client-side layer for placing elements in
time, kept frozen and taking no new work; it has no view and nothing is designed
around it. See [`clausters.form`](form.md).

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

```python
from clausters.multitrack import Arrangement, Content, Lane, Region, Tempo, Track

piece = Arrangement()
piece.set_tempo(Tempo(at=0.0, bpm=96.0))

drums = Track(id=1, name="drums", lanes=[Lane(id=2)])
drums.active_lane.place(Region(id=3, position=0.0, length=4.0,
                               content=Content.onto(take)))
piece.tracks.append(drums)

written = piece.write()          # the crate's JSON
piece = Arrangement.read(written)
```

A **region** is the model's word and a **clip** is the picture's: a clip, a lane
row, a waveform are what the host draws; a region is what an edit names. And
everything placed is placed in **beats**, while what fills a region is measured
in its own source's units — the two are not the same axis, and the conversion
between them needs the tempo map, which is why the map is part of the piece.

### Editing a piece: the verbs a multitrack admits

The piece has an edit vocabulary of its own, and it is reached through the same
door every other structure is — `domain_edit`, with `ARRANGEMENT` as the
vocabulary. Hand over the piece as the crate's JSON and the edit; take back the
piece as it now stands and the edit that puts it back.

```python
from clausters.multitrack import Arrangement
from clausters.document import ARRANGEMENT, domain_edit

edited = domain_edit(
    ARRANGEMENT, piece.write(),
    {"intent": "placeregion", "region": 3, "track": 1, "lane": 2,
     "position": 16.0, "layer": 0},
)
edited["applied"]                       # True
Arrangement.read(edited["state"])       # the piece with the region moved
edited["current"]                       # the edit that puts it back
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

A `Multitrack` is one of the fundamental structures, so it opens with the same
verb the others do — and what opens is the multitrack **editor**: one widget
drawing its own ruler, its own track headers, its own automation rows and its own
boxes, over the same picture and the same reading of a gesture the standalone
host uses.

```python
session.gui()
editor = edit(piece, sample_rate=48_000.0, server=session.server,
              sources={1: take, 2: other}, title="piece")
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
`pause()` and `stop()` are the same three verbs from a script. A piece opened
with no server still edits; it is simply not heard.

Two cursors, and only one of them is placed: a click on the ruler — or on the
slack between boxes — puts the **position cursor** down, which is where the next
play starts, and a stopped transport is cued there. The playhead is never placed,
so stop goes back to the mark rather than to the top.

**A curve is drawn and not yet heard.** A track's level, its mute and its solo
reach the readers; the automation drawn on a track and the envelope drawn inside
a box do not, because a curve's `gain` and the header knob's `gain` have to name
one parameter of one node first. Until that lands the curves are edited, undone
and saved with the piece like everything else.

`examples/editors/edit_multitrack.py` is the whole of it, by ear and by eye.

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

```python
from clausters.multitrack import Session, Span, View

window = View(name="arranger", visible=Span(0.0, 48.0), quant=4.0)
window.track_view(10).height = 96.0
window.track_view(10).lanes_shown = True      # comping open
window.selected = [20, 32]

session = Session(arrangement=piece, views=[window])
```

**There is more than one of them.** A piece drawn in two windows has two views
and they disagree on purpose — the arranger snapping to a bar, the editor below
it to a sixteenth — which is why a session carries a list rather than a view.

**A view entry for something the piece no longer holds is dropped.** `prune`
does it, and the rule is the one this project already fixed a class of defects
by adopting: state goes when the thing goes. Keeping it is worse than losing it,
because a height kept for a track that is not the same track is a defect that
looks like a feature.

```python
window.prune(piece)      # True when something went
```

**What a view never reaches.** Not the document, and not the history: a view is
not edited through an intent, an undo never puts a scroll back, and nothing here
is consulted when an edit is applied. A session file may carry one because
reopening a piece into the window it was left in is what every program in the
field does — and a reader that ignores the field opens exactly the same music.

## The document: what the composition *is*, and who edits it

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the web client, and a GUI host running standalone with
no language attached at all. That is not an implementation detail you can ignore
once you edit from more than one place, so this section says what crosses.

**`clausters.form` has no door to it, and that is deliberate.** It had one until
2026-09-06 — a bridge that converted its elements to the crate's JSON — and it
was removed with the turn that made the arrangement a model of its own. What a
piece is written with now is `clausters.multitrack`, above; what the crate's
own document holds is a **leaf as an id, a kind and a configuration it never
interprets**, and a generator travels as a *reference* the way a project file
references a plugin rather than serializing it. A generator *is* code, in the
language that wrote it, so no format owns one; what the document guarantees is
that it does not lose it.

### An edit is applied in one place

`clausters.document` is the door to it — the same surface the web client reaches
through its own `document` module, name for name. The crate is the only thing
that applies an edit. A client does not apply and
then report — it hands over the document and the **intent** and receives the new
document plus what happened:

```python
from clausters.document import apply_intent

result = apply_intent(
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

An editor's undo is not the editor's. The history lives in the same crate as
the document, beside the data it inverts, and `undo` / `redo` step through it:

```python
# after an edit in the window
editor.can_undo                  # True
editor.undo_label                # "move the point"
editor.undo()                    # it springs back, and the window is told
editor.redo()                    # and forward again
```

Wire them to two buttons the way the transport is wired, by name:

```python
win["undo"].on_event(lambda v: editor.undo() if v == 1 else None)
```

The keyboard needs no wiring at all: **Ctrl+Z** and **Ctrl+Shift+Z** over the
window reach the same history, because the host sends them as an `"undo"`
addressed to the *window* rather than to a widget — undo is aimed at no place
under the cursor — and the editor answers it like any other event.

**Why the history is not kept here** is the whole reason it is worth explaining.
A log an editor keeps sees only the gestures *that editor* made — so a script
that edits the data, a second view on the same structure, or a re-render leaves
it describing something that has moved on, and undoing then writes a state
nobody was ever in. One history per structure, wherever the edits come from, is
the only version of this that stays true.

So the history belongs to the **data**, and two windows over one structure find
the same one:

```python
first = edit(curve, sample_rate=sr)
second = edit(curve, sample_rate=sr)      # a second window, same structure
                                          # drag a point in either window
second.can_undo                           # True: it is showing the data that moved
second.undo()                             # and the point springs back in both
```

**Every message reaches every editor**, and that is what the host's event loop
does with them: each open editor is handed the whole stream and answers for the
widgets it drew, so another window's events fall through untouched. An editor is
driven by `apply` rather than by `pump` — `pump` dispatches to the widget handles
a script registered — and the loop calls the first before the second, in one
order.

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
# after sweeping a marquee on a lane
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
# Ctrl+X over a selection covering a clip
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
from clausters.multitrack import Session, Source

session = Session(arrangement=piece, provenance={"script": "song.py"})
session.sources[7] = Source.file("takes/vocal.wav", lifetime="external")

written = session.write()
session = Session.read(written)
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

A buffer read from a file is written as that file; one allocated in this run is
written **volatile** (`Source.volatile()`) — it existed only while the process
did, and a session that promised otherwise would reopen with silence where it
promised samples. A path inside the session's own folder is written relative, so
the pair of files moves together; one outside it stays absolute, because a
session never claims to own your file.

Three questions a save asks the table, and each has an answer rather than an
exception: `session.volatile()` is what is not written down anywhere,
`session.open_edits()` is what is still undecided, and `session.dangling()` is
what the piece names and the table does not hold — **every** lane walked, not
only the ones that play, because an alternate take names its source whether or
not anyone has chosen it yet.

### Reopening: structures, not a description

`Session.read` gives the piece and its table back, and by itself that is half a
verb: every take is a bare source number and nothing has loaded it. Resolving
the table is the other half, and it is the caller's, because what a source *is*
in a running system — a buffer to allocate, a file to map — is not the
document's to decide:

```python
from clausters.multitrack import Session
from clausters.defs import Buffer

with open(path) as f:
    session = Session.read(json.load(f))

buffers = {id: Buffer.read(os.path.join(folder, source.path), server=server)
           for id, source in session.sources.items() if source.path}
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

### What a piece is as nodes: the channel strip, three times

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
reference to whoever can resolve it and takes back whatever that caller has,
which means the reference must be something you can produce on the way back
in. A def and an
automation carry a name of their own and need nothing; a pattern does not, so
name the **element**:

```python
bass = Sequence(Pbind(midinote=Pseq([48, 55], 2), dur=1.0), name="bassline")
```

A name is a label, not an identity: nothing addresses an element by it, and two
elements may share one — which is what naming *the same algorithm used twice*
looks like. An **unnamed** leaf is written with no reference at all and comes
back **frozen**: drawn, placed, silent, contributing its extent and emitting
nothing. That is not the file being lossy; it is what a composition means
somewhere its language is not running, and it is what a `standalone` host with
no interpreter shows for every generator in the piece.

The same name is what a view labels a track with, so naming a track
is worth doing before it is worth needing.
