# The document: what a multitrack is, and who edits it

A `Timeline` places items at beats and plays them. That is enough to
sequence, but not enough to *compose*: a multitrack places recorded and
generated contents in time, with tracks, takes and curves that are **authored,
durable and undoable** — state
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

## What an editor is

`clausters.gui.editing.Editor` edits **one structure** — a buffer's samples, a
break-point curve, a timeline of events — and it knows nothing about any
arrangement. That is the whole of it, and it is deliberately the plain case:
editing a curve is what an editor is for.

An editor orchestrates rather than performs, and it is four collaborators
(`clausters.gui.editing`):

| | what it is | what it deliberately is not |
|---|---|---|
| `View` | the picture of one structure, and the registry from widget id to what it shows | not the vocabulary: one structure is drawn several ways |
| `Domain` | what a gesture needs read with it, and the applied payload written onto the client object | not **what a gesture means** and not **how an edit inverts** — both are the shared crate's, so neither is written once per language — and it does not draw |
| `Echo` | the acknowledgement: the stamp, the version, the corrections, the reason | not anything about what was edited |
| `Editing` | the editing context: the history, and the views to tell | **not the editor's** — it is asked for, never built, which is what makes two windows walk one undo order |

The rule that fixes all four: an editor owns **neither the data nor the
history**. `View` here is not `clausters.gui.guidef.View`, which is a tree you
can open.

## `edit(x)`: one verb over the four structures

`clausters.gui.edit` opens whichever editor the structure asks for, and it
dispatches on **what the structure holds**, never on its class — a curve editor
asks for break points it can read and write back, and everything that answers
opens, which is why the curve row below names three unrelated types:

| `edit(x)` where x is | opens | over | its vocabulary |
|---|---|---|---|
| a `Buffer` | `AudioEditor` | a `waveform` | `parts` |
| a curve — a `Bpf`, an `Env`, a `multitrack.Automation` | `PointsEditor` | a `bpf` | `points` |
| a `Timeline` | `NotesEditor` | a `pianoroll` | `events` |
| a `Multitrack` | `MultitrackEditor` | a `multitrack` | `clips`/`lanes` |

**A `Buffer` opens in the audio editor**, `clausters.gui.editing.AudioEditor`.
It writes nothing it was handed while it edits. Its window draws a **join** the editor owns, and
every edit -- a cut, a paste, a mix (Ctrl+Shift+V), a pencil stroke -- leaves a
new list of spans over the take and over the takes the edits made: a stroke is a
new take the size of the stroke, spliced over the frames it was drawn on. An undo
is the list before, stitched again, so it costs the list and not the samples.
The takes a history can still reach are kept, and freed when it cannot;
`history_bytes=` caps what only the history holds, and `resident_bytes=` how much of that stays in memory -- past it the oldest takes are written to a `scratch=` directory and read back when an undo reaches them. What it opens is a file or a server buffer, and it edits a private copy of it: `editor.save()` writes the edited take over what it was opened from -- the file it was read from, or the buffer, rewritten whole at the take's length -- and Ctrl+S in the window does the same. `editor.save(path)` writes it as another file and `editor.save(buffer=b)` into another buffer (`buffer=True` for a new one), which a later save then writes over. `editor.buffer` is the edited
take, to play or read, and `editor.parts` what it is made of.

```python
from clausters.gui import edit

editor = edit(curve, sample_rate=48_000.0)

curve.to_points()      # the edited curve, out of the object you already held
```

**It opens.** The window is up and listening when that returns, so the
structure you already hold is the edited one from that moment: read it whenever,
and it says what the hand has left there. `open=False` builds the editor without
a window, for a caller composing one.

Nothing is handed back: the object passed in *is* the edited one -- except a buffer, which the audio editor writes back when it is saved. A
**multitrack** is one of them, and what opens is the multitrack editor — the
rest of this chapter is what it edits.

Reading it back **after the hand is done** is `wait`:

```python
editor.wait()          # returns when the window is closed
curve.to_points()      # the curve, as it was left
```

`editor.closed` is the same question asked without waiting, and
`editor.close()` closes the window from the script — the history is not closed
with it, since an undo order belongs to the data.

**Two calls over one structure give two windows and one stack.** The editing
context belongs to the data, so an undo in either window steps the one order both
of them made. And a window composing several structures passes one context
(`edit(x, context=...)`), which is what makes it undo across a curve and a roll
in the order the edits happened.

**How an edit inverts is the shared crate's.** For a curve and a timeline the
state goes in with the payload and comes back as what the structure now is *plus*
what puts it back — one call, because the inverse has to be read before the edit
lands. A span of samples is the exception, and a real one rather than an
omission: the frames are in a server buffer, so the crate holds no state to
invert. What it shares there is the payload's shape and its coalesce key, and the
inverse rides on the wire — a stroke's event carries the run it wrote *and* the
run it replaced. This client writes a stroke's samples synchronously; a page's
buffer calls are asynchronous, so the web client queues them in order instead,
and that is the only difference between the two.

## The arrangement: tracks, lanes, regions

The model a multitrack editor edits is the one the three classic
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
- An **automation** is a curve over one parameter, in the multitrack's time.
- The **multitrack** is the tracks plus what there is one of: the tempo
  map, the meter map, the markers, the loop. They are there and not on a track
  precisely so that no two tracks can disagree about them.

```python
from clausters.multitrack import Content, Lane, Multitrack, Region, Tempo, Track

multitrack = Multitrack()
multitrack.set_tempo(Tempo(at=0.0, tempo=1.6))       # beats per second: 96 a minute
bar = multitrack.tempo_map().secs_at(4.0)            # where the second bar begins

drums = Track(id=1, name="drums", lanes=[Lane(id=2)])
drums.active_lane.place(Region(id=3, position=bar, length=2.5,
                               content=Content.onto(take)))
multitrack.tracks.append(drums)

written = multitrack.write()          # the crate's JSON
multitrack = Multitrack.read(written)
```

A **region** is the model's word and a **clip** is the picture's: a clip, a lane
row, a waveform are what the host draws; a region is what an edit names. And
everything placed is placed in **seconds** — a region, its fades, a curve's
points, a marker, the loop — while what fills a region is measured in its own
source's units, so the two are not the same axis. The tempo and meter maps are
structures the multitrack holds rather than its axis: a ruler draws beats and bars
from them and a snap reads them, and an edit of the tempo moves no region. A
session saved in beats, before this, is converted when it is read.

### Editing a multitrack: the verbs a multitrack admits

The multitrack has an edit vocabulary of its own, and it is reached through the same
door every other structure is — `domain_edit`, with `MULTITRACK` as the
vocabulary. Hand over the multitrack as the crate's JSON and the edit; take back the
multitrack as it now stands and the edit that puts it back.

```python
from clausters.document import MULTITRACK, domain_edit

edited = domain_edit(
    MULTITRACK, multitrack.write(),
    {"intent": "placeregion", "region": 3, "track": 1, "lane": 2,
     "position": 16.0, "layer": 0},
)
edited["applied"]                       # True
Multitrack.read(edited["state"])        # the multitrack with the region moved
edited["current"]                       # the edit that puts it back
```

Fourteen verbs, in three groups. What a **track** is: `settracks` (the tracks
now, whole — adding, removing and reordering are one verb, because all three
say the same thing) and `setactivelane`, which is comping's one verb. What a
**region** is: `setlane` (a lane's regions, whole), `placeregion`, `trimregion`,
`splitregion`, `joinregions` and `faderegion`. What the **multitrack** holds:
`setautomation`, `setmarker`, `removemarker`, `setrange` (the loop or the punch
span), `settempomap` and `setmetermap`.

Three things about them are worth knowing before you write against them.

**Moving a region to another track is one edit.** Where a region is means track,
lane *and* second, and an intent is absolute — so `placeregion` states all three
together. One entry in a history, one undo, and no moment in between where the
region is on no lane at all.

**A crossfade is two fades over an overlap**, not a third object: `faderegion`
on each of the two regions, which is what the model already holds. There is no
crossfade to lose track of, and nothing to keep in step with the two fades.

**A split and a join ask you for the content.** They are the two edits that
change how many regions there are, and the two the crate will not work out on
its own: the cut is on the multitrack's axis and a window into a source is on
the content's — a recording's seconds read at a playrate, a node's own beats —
and this document converts between the two *never*. So `splitregion` takes
`left_content` and `right_content` and `joinregions` takes `content`, from you,
who knows how the content is read. Omit them and both halves go on reading what
the region read.
Both also invert as `setlane`, the lane's previous contents: nothing smaller
describes putting back a region that was made out of two.

Everything else the vocabulary does it does the way the tree's does — absolute,
idempotent, refused rather than merged when it was made against a multitrack that has
moved. The multitrack carries **its own version** for exactly that: an editor of the
multitrack is not editing the tree, so one counter for both would make every edit to
either look like a change to both.

## Editing a multitrack: `edit(multitrack)`, and what it plays

A `Multitrack` is one of the fundamental structures, so it opens with the same
verb the others do — and what opens is the multitrack **editor**: one widget
drawing its own ruler, its own track headers, its own automation rows and its own
boxes, over the same picture and the same reading of a gesture the standalone
host uses.

```python
session.gui()
editor = edit(multitrack, sample_rate=48_000.0, server=session.server,
              sources={1: take, 2: other}, title="multitrack")
```

`sources` is the one fact about a multitrack that is not in the multitrack: the document
names a **source id**, never a path and never a buffer number, so which buffer
each source was read into travels beside it.

**Given a `server`, the multitrack sounds.** The editor keeps one resident reader per
box in a group the server's transport governs, and puts them where the multitrack says
on every edit whoever made it — this window's gesture, a second window over the
same multitrack, or a step of the history. Moving a box while it plays is one
`/node_set` on a node that is already running, so it is heard where it was
dropped with nothing that is sounding cut. The window carries the transport row
that goes with it (rewind, play/pause, stop, and where the multitrack is), and
`editor.play()`, `pause()`, `stop()` and `rewind()` are the same verbs from a
script. A multitrack opened
with no server still edits; it is simply not heard.

Two cursors, and only one of them is placed: a click on the ruler — or on the
slack between boxes — puts the **position cursor** down, which is where the next
play starts, and a stopped transport is cued there. The playhead is never placed,
so stop goes back to the mark rather than to the top -- and **rewind** is what
puts the mark itself back at the top, which is a statement about the cursor and
not about the transport.

**A curve is heard.** A track's level, its mute and its solo reach the readers,
and so do the automation drawn on a track and the envelope drawn inside a box:
each names the `gain` port of the node it is on -- the same port the header's
knob writes -- so a point dragged while the multitrack plays is heard where it is
drawn.

**And a track shows what it produces.** The strip in each header is one column
per channel over that track's own output, after its clips, its curves and its
fader. It is read in decibels, which is what makes it legible: the column is
green up to the alignment level (-18 dBFS), ambers through the headroom above
it and is red in the last six decibels before full scale, with the peak it
reached held beside it.

`examples/editors/edit_multitrack.py` is the whole of it, by ear and by eye.

## The presentation: what a window shows, beside what the multitrack is

Where a window is looking, how far it is zoomed, what the hand is holding, how
tall each track is drawn — none of that is what the multitrack *is*, and all of it is
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

session = Session(arrangement=multitrack, views=[window])
```

**There is more than one of them.** A multitrack drawn in two windows has two views
and they disagree on purpose — the arranger snapping to a bar, the editor below
it to a sixteenth — which is why a session carries a list rather than a view.

**A view entry for something the multitrack no longer holds is dropped.** `prune`
does it, and the rule is the one this project already fixed a class of defects
by adopting: state goes when the thing goes. Keeping it is worse than losing it,
because a height kept for a track that is not the same track is a defect that
looks like a feature.

```python
window.prune(multitrack)      # True when something went
```

**What a view never reaches.** Not the document, and not the history: a view is
not edited through an intent, an undo never puts a scroll back, and nothing here
is consulted when an edit is applied. A session file may carry one because
reopening a multitrack into the window it was left in is what every program in the
field does — and a reader that ignores the field opens exactly the same music.

## The document, and who edits it

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the web client, and a GUI host running standalone with
no language attached at all. That is not an implementation detail you can ignore
once you edit from more than one place, so this section says what crosses.

**`clausters.form` has no door to it, and that is deliberate.** It had one until
2026-09-06 — a bridge that converted its elements to the crate's JSON — and it
was removed with the turn that made the arrangement a model of its own. What a
multitrack is written with now is `clausters.multitrack`, above; what the crate's
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

That holds for a window over a *part* of the multitrack too — a dedicated roll of one
track edits through the multitrack's history rather than opening a second one
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

A sweep on a lane is not an edit — nothing in the multitrack changes — but it
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

The other scrap of screen state a driver can ask for is **which layer of a clip
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

session = Session(arrangement=multitrack, provenance={"script": "song.py"})
session.sources[7] = Source.file("takes/vocal.wav", lifetime="external")

session.save("song.json")
session = Session.open("song.json")
```

`save` writes the crate's JSON and `open` reads it back; `write` and `read` are
the same two steps without the file, for a caller that keeps it elsewhere.

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
what the multitrack names and the table does not hold — **every** lane walked, not
only the ones that play, because an alternate take names its source whether or
not anyone has chosen it yet.

### Reopening: structures, not a description

`Session.read` gives the multitrack and its table back, and by itself that is half a
verb: every take is a bare source number and nothing has loaded it. `load` is
the other half — what a source *is* in a running system is not the document's to
decide, so it is said by loading the table into a server:

```python
from clausters.multitrack import Session

session = Session.open(path)
buffers = session.load(server)
```

The answer is a `Buffer` per source, keyed by source id — the same table an
editor takes as its `sources`. Each take is read from its file **once per
source**, a relative path against the folder the session was opened from: two
clips over one take are two windows onto one buffer, and reading
it twice gives them two buffers that drift apart on the first edit. A **join**
is stitched from the takes it is made of, after they have loaded, so it plays
the spans the file states; a take only a join reads is loaded for it. What is
read and in what order is the shared crate's, so the web client and the GUI host
open the same session the same way.

A *volatile* source is left out with a warning rather than returned as a lie,
and the rest of the multitrack opens: half a session is worth opening. A file that is
not there is the server's refusal of its read, and `load` raises after freeing
what it had made. And a generator whose reference `defs` does not have keeps what
it last **rendered** as its floor, which is the same thing a host with no
language attached shows.

### Mixing is the multitrack's

Every element carries `mute`, `solo` and `level`, and all three are inherited
down the tree: muting an aggregate silences its members, one soloed element
anywhere silences every branch that is not on a soloed path, and a level
multiplies into the `amp` of the events under it.

```python
bass_lane.mute = True
lead_lane.level = 0.5
```

They ride in the node's **configuration**, so a multitrack reopens mixed the way it
was left, and the editor's lane header is drawing the multitrack rather than
remembering something of its own — pressing mute there goes through the log and
undoes like any other edit. What is *drawn* is read unmixed: a muted lane keeps
its clips, its notes and its length, because a picture that emptied when the
toggle was pressed would report silence as absence.

A lane's **height** is the other kind of thing and is in no document. It says
nothing about what the multitrack is; resizing a lane (Ctrl+wheel) changes the view
and no file.

### What a multitrack is as nodes: the channel strip, three times

A multitrack is a picture and a sound, and this is the second one. What plays it is
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

The multitracks of it are named once, in `clausters_core::mixer`, and both clients
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
the mix and reopening a multitrack has to give back the mix it was left with.
A track's fader is `Track.level`, a field for the same reason: what a multitrack
sounds like is the multitrack's, not a key one client reads out of a table it was
only meant to carry.

A strip writes an output bus of its own and a **send** carries it onward at a
gain -- one node between a track and the master. That is what lets a meter mean
something: every track writes into the master's mix, so a meter there would read
the sum and call it the track. A meter is a **slot** on a strip, so a multitrack
nobody is looking at holds none, and each one writes one control bus per channel
-- one number a block, with the fall and the peak hold applied on the server, so
two clients cannot draw two different falls off one signal.

And the transport plays the multitrack rather than a client stepping it: each reader
reads the position the engine publishes, so moving a box is one `set` and a
locate is no message at all.

### A cut that plays as one reader

A box is a window onto a buffer, and a cut assembled from several takes -- or
from one take in another order -- is not that. `Buffer.stitch` installs a
**join**: a buffer whose samples are spans of other buffers, read as one.

```python
from clausters.defs import Part

cut = Buffer.stitch([Part(verse, 0, 2 * 48000), Part(chorus, 4800, 48000)],
                    server=server)
```

Without it a reader would have to change which buffer it reads with sample
accuracy, and the buffer a reader reads is an initial-rate control: every seam
would be a new node and a control message in the middle of playback -- which is
exactly what a multitrack that plays itself from the transport must not need. The
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

### Where a recording lands

A `Buffer` holds a take and a `RecordingStream` follows one as it is written,
and neither puts one in a multitrack. `take` does:

```python
from clausters.form import take

song.add(take(recorded, instrument="player"), offset=8.0)
```

It is a `Vector` whose length is the samples' own — frames over the rate they
were recorded at — which is the one line every script used to write by hand.
Without an `instrument` it is structure: it draws and it extends the multitrack, and
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
nothing. That is not the file being lossy; it is what a multitrack means
somewhere its language is not running, and it is what a `standalone` host with
no interpreter shows for every generator in the multitrack.

The same name is what a view labels a track with, so naming a track
is worth doing before it is worth needing.
