# The application scope - implementation plan

*Opened 2026-09-06 with the user, on the branch `application-scope`. **This file
is temporary.** It exists so one refactor spanning four packages can be carried
out against a written sequence instead of against memory; it is a record of
nothing. When the branch merges and the work is proven, the file is deleted -
and the last milestone below is what makes that legal, because a decision worth
keeping is not kept here. The `AP` labels are this file's own coordinates and
appear nowhere else; they are not roadmap labels and must not reach a published
doc, a docstring, an example or a comment.*

## What this branch is

**The editor stops being the unit, and the application becomes it.**

Today `clausters.gui.editing.Editor` owns everything global about a session on
screen: the host, the window, the widget-id pool, the acknowledgement, and the
screen state (selection, zoom, layer). What is actually shared by several views
lives one level down, in `Editing` - the context the *data* owns: the history,
the version, the list of views to tell. `FormEditor` is `Editor` plus a document,
and `FormEditing` is `Editing` plus a node index.

That arrangement has three consequences this branch is about:

- a widget id is a **lease**, not an identity, so anything in flight across a
  redraw lands on the wrong widget;
- everything global is written **once per language**, so the two clients carry
  ~5 800 (Python) and ~6 700 (TypeScript) lines of `editing/` that are one
  logic in two spellings;
- there is no layer that means "an application", so a second one - a bundle of
  editable subviews that is not the multitrack - has nowhere to be built, and
  the open question in `crates/clausters-document/PLAN.md` ("What is the second
  document: the application, and not the arrangement?") has nothing to be
  answered against.

The inversion: **an application owns the host, the id space, the acknowledgement,
the screen state and the undo order; an editor is one structure bound to one
`Domain` and one `View`, and owns nothing global.** `FormEditor` is then not a
subclass of `Editor` - it is an *application* whose structure is a document and
whose views are lanes and clips, and it becomes the second consumer of the
abstraction rather than the place it is hidden.

## What is already right, and is not to be re-derived

Stated because half of this refactor is *not* moving things that already sit
well, and a pass over this code will otherwise "fix" them:

- **The inverse is the crate's.** `history::Editable` (`apply`, `current`,
  `coalesce_key`) is bound by both clients; `PointsDomain.current`/`project` go
  through `clausters_domain_edit`, the coalescing key through
  `clausters_domain_coalesce_key`, a curve's drawn axis through
  `clausters_core_curve_axis`. No client re-derives an inverse.
- **The history belongs to the data** (`Ox` O15-O19). `Editing.of` caches the
  context on the structure; `clausters_history_register` mints a structure's
  identity in Rust. Two windows over one thing walk one order.
- **`Domain` and `View` are separate, and the reason is right**: one structure
  is drawn several ways while its vocabulary is one.
- **The builtins are the core's.** `midicps` and the rest reach both clients
  through `clausters_core_unary`/`_binary`/`_map` and the slice forms; nothing
  here changes that, and nothing here should add a second numeric path.
- **`GuiNode` has one construction path.** `clients/gui/src/tree.rs` builds the
  same node the JSON parser produces, deliberately, so a Rust-side projection
  has a door already.

## Non-goals

- **No second owner of a document.** The version counter is enough precisely
  because one owner is authoritative per resource; nothing here moves toward
  operational transformation or CRDTs (`Ox`, "More than one owner of the same
  document").
- **No shared mutable memory as a model.** Shared memory stays what it is: a
  native fast path under an ownership rule that is already decided. The browser
  has no mapping and the web client is first-class, so a design that needs shm
  to be correct is a design that has already diverged.
- **The opaque leaf stays opaque.** Nothing here lets the document interpret a
  generator's configuration. A projection that needs to draw one asks its owner
  for a summary; it never grows a case for `Pbind`.
- **No new widget, no new gesture, no new drawing rule.** This branch moves
  where code lives and what a widget id means. If it grows a visible feature,
  that feature has escaped its own milestone.
- **`clausters.form` keeps its surface.** Users of the arrangement API should
  not be able to tell this happened, except that ids stop appearing.

## Design reference: what the field does, and where we differ

Read 2026-09-06, after AP5's conclusion was already reached, to check it against
prior art rather than to derive it. Sources at the end. It is here and not in a
milestone because it is **context the milestones are read against**, and because
none of it is work: an entry that turns into work goes to "Found by use" or into
a milestone, named.

**Item and Composition — one hierarchy, two categories.** OpenTimelineIO splits
`Composable` into an **Item** (a leaf: clip, gap, transition) and a
**Composition** (a container: track, stack, timeline). A `Track` orders its
children **sequentially in time**; a `Stack` orders them **in parallel**, over
the same range.

*Ours is a different axis.* `Aggregate`'s two kinds are **concrete** (its members
relate in time) and **logical** (they relate by processing) — sequence-versus-
layer is not the distinction we drew, and we do not have it. Worth knowing we
chose one axis and not the other; not worth changing on this evidence.

**Empty space is an object.** In OTIO a `Gap` is a real item, so a track is a
**sequence** whose positions derive from the order. We use absolute offsets, as
Ardour does. That is the NLE family (sequence plus gaps, ripple editing falls
out) against the DAW family (free placement). **We are in the right family for
what this is**; the entry exists so the choice is known to be a choice.

**Source -> Region -> Playlist -> Track: four levels, not two.** Ardour's is the
model this branch most lacks a piece of. A **Source** is the immutable file; a
**Region** is a window onto a source, shareable, so one region may appear in
several places; a **Playlist** is the ordered list of regions that **is a
track's contents**; a **Track** plays a playlist, and playlists are
**swappable** on a track — which is what takes and comping are.

*The playlist is the indirection we do not have.* In Ardour a clip changing
track is *remove from playlist A, add to playlist B* — a list operation — and
the track keeps its identity and its state with no regard to its contents. Here
a lane's contents **are its children in the widget tree**, so the same gesture is
`reparent_clip`. That is the difference between the roll and the multitrack,
arrived at from the other side: the roll's contents are data in one widget, and
that is why the whole class of id and screen-state defects cannot occur in it.

**A clip's three numbers are the same three.** OTIO carries `source_range` and
derives `trimmed_range`/`visible_range`. Our `offset`/`dur`/`start` are those,
with `start` the source range. **We match**; nothing to do.

**The model is authoritative and the view is generated and reconciled.**
kdenlive keeps the truth in a C++ `TimelineModel` and draws with QML
(`Track.qml`, `Clip.qml`), and between them each track holds a **`DelegateModel`**
which binds the visual representation to the model and *"eliminates manual view
synchronization"* — the framework decides which delegate appears, which goes, and
which merely updates. Ardour does the same by hand: the canvas's `RegionView`s
are built **from** the playlist, and the view owns them.

*This is AP5's conclusion under its industry name.* Reconciliation is the
standard answer, Qt hands it over for free, and we would be building it. The
agreement is worth recording precisely because we got there from a bug report
rather than from a book.

**The split of state ownership is the one we already drew.** kdenlive: model =
positions, durations, structure; view and controller = selection highlight, drag
previews, the selected ids, the active track. That is our four-layer rule, and
this is a **confirmation, not a finding**.

**Identity by id, order by index, both.** Tracktion's `ClipTrack` finds a clip by
matching id *and* indexes by position. That is AP1's direction, and it is
ordinary.

**The one thing nobody does is what we do.** Every system here keeps model and
view in one process, so none of them sends a view tree over a wire per redraw.
Our nearest analogue is not a DAW at all — it is the DOM with a reconciler, or
Qt's `DelegateModel`. Both are reconcilers, which is the second time the same
answer arrives by a different road.

**What the reading adds that we had not said.** The open question at the end of
AP5 — whether a lane could carry its placements as a **prop**, a list of
`(id, offset, dur, start)`, with bodies as children only where there is one to
draw — has a name and forty years of use: it is a **playlist**. And it brings two
things this project does not have and has never designed: playlists swappable
over one track (takes, comping, alternate versions) and a clip appearing in more
than one place without the data being copied. *(We take the structure and not the
name — it is `Lane` here; see "The name: `Lane`, not `Playlist`" below.)* Whether we want either is not
settled here. What is settled is that the shape was not invented in this
conversation.

### REAPER, read through its public surface (2026-09-06)

REAPER is **not open source** - it is proprietary, and no source was read. What is
public and is better for this anyway: the **ReaScript API**, the **extension
SDK**, and the **`.rpp` project format**, which is plain text and has several
independent parsers. The model reads more clearly there than it would in source.

**Its object model is five levels, not four.**

    Project -> Track -> MediaItem -> Take -> PCM_source

**Two confirmations, from a third independent system.** The item carries
`POSITION`, `LENGTH` and a **source offset**, manipulable separately - which is
what makes slip editing possible, and is our `offset`/`dur`/`start` again after
OTIO's `source_range` and Ardour's region. And **automation attaches at three
levels, not one**: `GetEnvelopeInfo_Value` exposes `P_TRACK`, `P_ITEM` and
`P_TAKE` as an envelope's possible parents. `O21`'s "a lane per addressable
target (a track's parameter, a region's, a plugin's)" was a guess when it was
written and is now confirmed.

**The real difference: the take lives on the item, not on the track.** A
`MediaItem` holds several `MediaItem_Take`s with one active, where Ardour puts
the alternatives on the *track*. The extra level splits two things our `Region`
holds together:

| | what it is |
|---|---|
| **Item** | the **slot in time** - where it starts, how long, its fades |
| **Take** | **what fills it** - the source reference, its offset, its playrate, its own envelopes |

That is the same distinction the crate's open decision closed as *name the
placement*: the placement given an identity, and a reference to what it places
with the arguments of that evaluation. REAPER has it as two objects; we wrote it
as one object with a reference inside. **Whether to split it is an open question
for `O21`** - splitting gives comping by construction (swap the take, keep the
slot) and costs a level in the format and in every intent.

**And the finding that is worth more than any structure: REAPER changed its
mind.** REAPER 7 added **Fixed Item Lanes**, with an action named, literally,
*"Track properties: Fixed item lanes (convert takes to lanes)"* - and the lanes
are **separate items in parallel layers, not takes stacked inside one item**. So
after twenty years of takes-inside-the-item, doing real comping needed the lane
model added **on top**, and the old one could not be removed; both now coexist
and a user has to know which they are in.

**The lesson, and it decides one of our questions.** Takes-inside-the-item
answers *an alternative take* and does not scale to *assembling a composite from
several*. We design once and from nothing, so we take the lane shape - Ardour's,
and the one REAPER ended up needing - and not takes inside the item. That is
evidence rather than preference, and it is why this entry exists.

**A smaller difference, in the format.** In `.rpp` the `<SOURCE WAVE FILE "...">`
is written **inside the item**. Our session format does the opposite - a source
table plus references, as Ardour does. REAPER's is simpler to read and cannot
express *these six items share one source* without repeating the path, which is
the thing that makes non-destructive editing cheap. We keep ours, now with the
reason written down.

**Sources (REAPER).**
[ReaScript API](https://www.reaper.fm/sdk/reascript/reascripthelp.html) *
[`reaper_plugin_functions.h`](https://github.com/juliansader/js_ReaScriptAPI/blob/master/reaper_plugin_functions.h) *
[`rppp`, an RPP parser](https://github.com/CharlesHolbrow/rppp) *
[`rpp`, RPP in Python](https://github.com/Perlence/rpp) *
[Fixed Item Lanes and swipe comping](https://forums.cockos.com/showthread.php?t=283665) *
[Recording with Fixed Item Lanes](https://reaper.blog/2023/10/record-track-lanes/)

### The open implementations, read for how the picture relates to the logic (2026-09-06)

A second pass, over programs picked for what they do about *the view* rather than
about the model: LMMS, Zrythm, Sonic Visualiser, Ardour's canvas, and - through
their documented object models rather than source, since both are closed -
Ableton Live and Bitwig. Three of the findings became work and are recorded in
`crates/clausters-document/PLAN.md` (`O21`'s timebase types, `O23`'s view object,
`O24`'s panes and layers). The rest is here.

**A widget per clip is not the defect, and saying otherwise would have made us
over-correct.** LMMS has a `Clip` model and a `ClipView` widget per clip, the
model emitting `dataChanged`/`propertiesChanged` and the views following; it is
ordinary MVC and it works. What fails here is narrower: **the widget tree is the
only copy of the structure and it lives across a wire.** LMMS affords a widget
per clip because the model is authoritative and in-process. So `O23` does not
have to stop drawing a clip as a widget - it has to stop the widget tree being
the only place the structure exists. *(The correction is written into the turn
itself, where the claim was made.)*

**Zrythm is doing this same turn, now, and its layering matches ours.** Five
layers - UI (Qt/QML), application logic (undo through `QUndoCommand`), data
model, audio, plugins - plus **object registries: runtime lookup tables for
arranger objects**, which is `AP1`'s derived id under another name. Its 2026
overhaul renamed `Region` to `Clip`, unified a `Clip` base absorbing
`BoundedObject`/`LoopableObject`, and added `Position`/`Timebase` primitives with
strong `ContentTick`/`TimelineTick` types. Two things follow: the vocabulary is
still moving in the field, so ours being deliberate is not eccentric; and the
strong timebase types are the cheapest thing on this page, which is why they went
into `O21`.

**Live keeps view objects parallel to model objects.** `Song.View`, `Track.View`,
`Application.View` are separate objects beside their model objects rather than
children - presentation on one side, functional data on the other, both readable
and writable by a script. That is the four-layer table with the presentation row
made **addressable**, and it is what `O23`'s prerequisite becomes.

**And Live has two views over one model**: the same track is a column of
`ClipSlot`s in Session and a timeline of clips in Arrangement, with a `Scene` as
the horizontal grouping across tracks. Bitwig separates them by intent - arranger
clips sound at a designated time, launcher clips must be available whenever - and
nests with **group tracks and sub-scenes**, which is `O21`'s folder track. For us
this is the proof that *a view is configured by what it holds* scales to two
radically different pictures of one model, and an argument that the presentation
model is **per view** rather than one global thing.

**Ardour's canvas, for the day the timeline is long in earnest.** A custom
retained scenegraph: `Item`s with a `render()`, `Container`s that draw nothing
and render children, and `ScrollGroup`s that scroll independently (rulers
horizontally only, headers vertically only) through an O(1) pointer to the scroll
parent. Three coordinate spaces - Window, Canvas (about +/-1e307) and Item -
because Cairo is only reliable to about 32767 px, so everything converts to
window space before drawing; with a 64-bit timeline that problem is ours too.
And **no dirty-region tracking at all**: they lean on the window system's expose
events, and a change entirely off screen queues no redraw.

Worth keeping for the rule it states in someone else's words: they built it
without scaling, rotation or 3D because *"single pixels have semantic content
inherent in their existence and placement"* - which is this project's own
never-resolve-finer-than-the-screen rule, arrived at independently.

**Sources (this pass).**
[The Ardour Canvas](https://ardour.org/canvas.html) *
[LMMS architecture](https://github.com/LMMS/lmms/wiki/LMMS-Architecture) *
[LMMS `ClipView.cpp`](https://github.com/LMMS/lmms/blob/master/src/gui/clips/ClipView.cpp) *
[Zrythm architecture](https://deepwiki.com/zrythm/zrythm) *
[Zrythm MR !27, Region to Clip and the timebases](https://gitlab.zrythm.org/zrythm/zrythm/-/merge_requests/27) *
[Sonic Visualiser: A Brief Reference](https://www.sonicvisualiser.org/doc/reference/3.1.1/en/) *
[The Live Object Model](https://docs.cycling74.com/legacy/max8/vignettes/live_object_model) *
[Bitwig: the clip launcher](https://www.bitwig.com/userguide/latest/the_clip_launcher/)

### The name: `Lane`, not `Playlist`

**Decided 2026-09-06 by the user, and the reasoning is kept because the name is
in the format, in every intent and in both clients once it is written.**

Ardour's **playlist** is *the ordered regions on one track* - its contents, not a
list of tracks - and a track holds several and plays one, which is what takes,
comping and alternate versions are. The structure is right and **the name is
not**: in ordinary use a playlist is a list of songs, so the word has to be
decoded before it means anything here. Ardour inherited it from Pro Tools, where
it is equally opaque.

That runs straight into the project's own rules - *name the structure, not the
category*, and *a metaphor that has to be decoded is not an explanation*.
Adopting the field's vocabulary buys recognition for a reader who comes from a
DAW; here it would have imported that field's worst name.

**So the structure is Ardour's and the name is ours: `Lane`.** A track holds
several lanes and plays one; a lane is an ordered list of regions.

**The cost, stated rather than discovered.** `lane` is already used about
**1596 times across 70 files** in the host and 60 times in `docs/gui-protocol.md`,
in **two** senses: a track's row in the multitrack, and a *channel* row inside a
multichannel clip body (`graphics/track.rs`: *"stacks its lanes: a clip is a
picture of the contents and a stereo take..."*). Neither is the new one.

The first collision resolves rather than fights: a track with three lanes **draws
as three rows** when expanded and one when collapsed, which is exactly REAPER 7's
picture, so the model word and the view word become the same word about the same
thing. The second does not and has to be renamed - a channel row is a
**channel**, and calling it a lane was always a stretch. **That rename is part of
`O21`, not something to leave for whoever trips on it**, and until it happens
`lane` means two things in the host.

**Sources.**
[OpenTimelineIO, timeline data model](https://deepwiki.com/AcademySoftwareFoundation/OpenTimelineIO/2.2-timeline-data-model) *
[kdenlive, timeline UI](https://deepwiki.com/KDE/kdenlive/3.2-timeline-ui) *
[Ardour manual, working with regions](https://manual.ardour.org/working-with-regions/) *
[Ardour, `route_time_axis.h`](https://community.ardour.org/files/doxygen/route__time__axis_8h_source.html) *
[Tracktion Engine, `tracktion_ClipTrack.h`](https://github.com/Tracktion/tracktion_engine/blob/master/modules/tracktion_engine/model/tracks/tracktion_ClipTrack.h) *
[Tracktion Engine, `Edit`](https://tracktion.github.io/tracktion_engine/classtracktion_1_1engine_1_1Edit.html)


## The milestones

### AP0 - The seam, in Python only, with nothing moved

Introduce the application/editor boundary where it is cheapest to move it, and
prove it with the editors that already exist before a line of Rust is written.

- A new `clausters.gui.editing.Application` (name provisional until AP8's doc
  pass; it is the object a window set belongs to). It takes over, from `Editor`:
  the host handle and its resolution (`_resolve_host`), the id space, the `Echo`,
  the poll/wait/close surface, and the undo/redo stepping that walks composed
  views (`_step`, `project_legs`, `reflect_step`).
- `Editing` becomes reachable **from** an application as well as from a
  structure: an application has one editing context, read off the editors
  registered in it when it was not handed one.
- `Editor` keeps `structure`, `domain`, `view`, the unit bridge, and `_route` /
  `_observe`. It no longer holds a host, mints an id or answers a host.
- `composed_in` / `composed_over` disappear as a mechanism: a composed editor is
  simply an editor registered in the same application.

**Acceptance:** `open_signal`, `open_pianoroll` and a bare `Editor` over a
buffer, a curve and a timeline all work unchanged from a script's point of view;
two windows over one structure still walk one undo order; the existing Python
tests pass with no change to their assertions. No TypeScript in this milestone.

**Done 2026-09-06.** `clausters.gui.editing.Application` holds the host and its
adoption rule, the widget-id space, the `Echo`, the socket drain and the walk of
the pile; `Editor` keeps the structure, the domain, the view and the unit
bridge, and reads the rest through `self.app` under the names it always used.
An editor handed no application makes one of its own, so nothing a script writes
changed - and `FormEditor` still works untouched, which is what lets AP6 converge
it rather than port it under pressure. 930 Python tests pass, `npx pyright` is
clean, `scripts/check-docs.sh python` builds, and `examples/editors/edit_curve.py`
opens and holds through the new `wait`.

**One deviation from the milestone as written, and why.** Registering a
structure at construction rather than on first edit was dropped: `Editor` admits
`structure=None` (a view inspected with no data behind it), and `Editing.of`
caches the context **on the object**, which `None` cannot carry. Minting stays
lazy in `_registered`, exactly as it was. What the milestone actually wanted -
that an application know its context without being told - is answered by reading
it off the registered editors, which costs nothing and has no such hole.

### AP1 - A widget id is named, not leased

The fix for the recurring visualization failures, and the thing every later
milestone depends on.

- A widget id is **named** rather than taken from a pool: it is asked for as
  `(structure identity, view role, key within the view)` and the same name gets
  the same number for as long as it keeps being drawn. The structure identity
  already exists and is already stable - it is what `clausters_history_register`
  mints and what `Editing.identity` caches - so no new notion of identity is
  introduced.
- `View.register` gains its inverse: a view can ask "what id draws this?" and get
  the same answer across redraws. The `widgets` map stops being rebuilt from
  scratch as the source of truth.
- The derivation lives in the shared crate from the start (a pure function over
  the key), so the two clients cannot derive differently. This is the one piece
  of Rust AP1 adds.
- `GuiIdAllocator` stays for hand-built GuiDefs, which have no structure behind
  them, and its docstring stops describing the multitrack's churn.

**Acceptance:** a redraw of any editor leaves every widget id unchanged for
every widget that still exists; an edit-back that crosses a redraw lands on the
widget the hand touched; a test drives a gesture, forces a redraw before the
answer is routed, and the picture and the data agree.

**Done 2026-09-06.** `clausters_core::widgetids::WidgetIds` is the GUI namespace
with **two doors over one occupancy map** - the anonymous lease a hand-built
tree takes, and the named id a view asks for - bound as `clausters_widgetids_*`
(C ABI v39) and `JsWidgetIds` (wasm), declared in `docs/bindings.md`. Both
clients' `GuiIdAllocator` sits on it; the Python `Application` takes the named
door, `View.widget(editor, role, showing, key)` is what a `build` calls, and the
three built-in views name their widget `"curve"`, `"waveform"` and `"roll"`.
`Editor.draw` brackets the draw so only what a picture genuinely stopped drawing
gives its id back. 935 Python tests, the core's 14 unit tests and its doctest,
`tests/bindings.rs`, `npx pyright`, the web `build.sh`/`test.sh`, the feature
matrix and `scripts/check-docs.sh` all pass.

**Four things this milestone learned, all of them recorded because they are the
kind of thing a later pass would otherwise "simplify" back out.**

- **A name is a map, not a hash.** A 31-bit space and a thousand live widgets is
  a collision every few thousand sessions, and a collision is two widgets
  answering to one number - silent, and indistinguishable from the bug the table
  exists to remove. The plan said "a pure function over the key"; a map costs a
  lookup and cannot do that, so a map it is.
- **A draw names its drawer.** One table serves a whole host, and a host carries
  more than one - two editors opened on the ambient host are two. A cycle that
  did not say whose draw it was let either one retire the other's widgets simply
  by redrawing, which is the same defect one level up. So `begin`/`retire` take
  an `owner` the table hands out, and `id_for` records who drew each name.
- **An anonymous free may not take back a named id.** A host frees a redefined
  subtree widget by widget (`_recycle_subtree`), and a keyed id is still held by
  its name at that moment; letting that free release it would hand one number to
  two widgets - exactly what was being fixed. A named id leaves only through
  `retire` or `forget`.
- **An unopened draw is private to whoever draws it.** With no host there is no
  window and nothing in flight, so such a draw starts its numbering over and
  drawing one picture twice gives one tree - the property `FormEditor`'s render
  tests rest on. On a host that would be wrong, because the leases there belong
  to every window the client has open.

**Where the port stands.** The core symbol reaches **both** bindings, and the
web `GuiIdAllocator` and `GuiHost.ids` moved with it, so the namespace surface
is in step. What is not ported is the door a view takes, because it sits on the
AP0 seam this branch deliberately did not write twice - `clients/web/PLAN.md`
carries the shape the port must follow, under "Parity gaps carried from the
Python client", and it closes with AP5/AP6.

### AP2 - A redraw is a diff

What AP1 makes possible and what stops the id churn at its source.

- An application computes the difference between the tree it last sent and the
  tree it would send now, and emits `/gui_set` for what changed plus definitions
  and frees for what appeared and went. A whole-tree redefine stays as the
  fallback and as what `open` does.
- Screen state therefore survives a redraw with no resync, because the widget
  was never freed.

**Acceptance:** editing one clip in a piece of many emits no definition and no
free; a scroll position and a selection survive a redraw of the window they are
in; the fallback path is still exercised by a test, since it is what `open`
uses.

**Mechanism done 2026-09-06; the acceptance is AP6's to finish, and the reason
is measured.** `Application.publish(window, tree)` sends the difference between
the tree the host is drawing and the one it would draw now: one `/gui_set` per
widget whose props moved, and a `/gui_def` only when the **shape** changed — a
widget that appeared or went, one that changed type or name, a prop that was
removed (the wire has no value meaning "unset"), or a tree carrying blobs. A
node with no id may stay as long as it is identical in both pictures, so the
chrome that carries none (a ruler, a spacer) does not make every tree holding
one a redefine. `published`/`forget_window` are the two bookkeeping doors, and
every redraw path now goes through them.

**What it does not yet buy, with the number.** Its only consumer is
`FormEditor`, whose widget ids are still **leased** — AP1 named the three
generic views and deliberately left the multitrack to AP6. On a real host two
draws of one composition therefore give different lane ids (measured: 20005,
20007 -> 20009, 20011), the shapes never line up, and a redraw of the *same*
piece still costs one whole redefine. Give the same editor a stable namespace
and the same redraw costs **zero definitions and zero sets**. So the mechanism
is right and idle, and what turns it on is naming the multitrack's widgets.

**That is a plan ordering error, recorded rather than worked around.** AP2's
acceptance sentence says "one clip in a piece of many", which is `FormEditor` —
so this milestone was always going to close inside AP6, and naming those ids
here would have meant doing AP6's convergence under AP2's name, including
changing what `draw` is allowed to do (a clip's stable key is its document node
id, and reaching one derives the document). AP6 takes the acceptance sentence
with it.

**One defect found on the way**, from AP0 and worth naming because nothing else
would have: an `Application` built with no `version` callable read its context
through `_editors` before `__init__` had made that list, so constructing one
standalone raised. Every editor passes a callable, which is why 935 tests never
touched it.

### AP3 - Screen state is about a thing, and a thing is not its address

**Rewritten 2026-09-06, because the milestone as opened contradicted a decision
this project had already made and written down three times.** It said that "two
views of one structure agree about the selection" and that `adopt_selection`
would go away. Both are wrong, and the tree says so in the book
(`clients/python/docs/src/composition.md`: *"What each window keeps for itself is
what a window can see: its selection, its zoom, which layer the hand is on"*), in
`Editing`'s own docstring, and in `examples/editors/two_windows.py` (*"the
selection does not travel"*). And `adopt_selection` is not the mechanism the
milestone thought it was: it is a **composed** view handing its selection *up* to
the composition it is part of, so an operation over a range is given one value
whichever of the piece's windows swept it. That is real and stays.

What survives of the milestone is its one true sentence - screen state is keyed
by *what it is about* - and it turns out to be a defect that was already in the
tree, in four places:

- Screen state was keyed by `id(object)`. CPython reuses an address the moment
  an object is freed - **196 times out of 200** in a straight loop - so such a
  table hands its state to whatever lands there next. Reproduced: expand an
  aggregate, let it go, make another, and the new one draws expanded. The same
  shape sat under a curve's held axis (`PointsView`), a patch's box placements,
  and - added by AP1 - an application's per-drawer id table.
- The fix is the one the tree already uses one level down (`Editing._structures`
  keeps the object beside the number "so its `id` cannot be reused"): key by the
  **object**, weakly. State then also goes when the thing goes, which is what
  screen state should do.

**Acceptance:** a new structure never inherits a freed one's axis, expansion or
id space; a selection is still readable from a script as a typed value
(`Ox` O6); nothing about screen state reaches a history or a file; and each
window still keeps its own selection, zoom and layer.

**Done 2026-09-06.** Four tables moved to weak keys (`PointsView._axis`/`_span`,
`FormEditor._expanded`/`_patch_geometry`, `Application._offline`/`_owners`), with
a test per failure that reproduces the inheritance. 944 Python tests pass.

**What is left where it was, deliberately:** the selection, zoom and layer stay
each window's, and the host stays their owner on the wire (`/gui_query` already
reports "what the widget is now"). Moving them out of the client is not what the
four-layer table asks for - the client keeps a *read-back value* a script uses,
not the authority - so there was nothing to move.

### AP4 - A payload is by value or by reference

The generalization of the asymmetry that exists today between `points` (whose
edit math crosses the ABI by value through `clausters_domain_edit`) and
`samples` (where it cannot, so the domain answers nothing).

- The application core takes payloads through one seam that says which of the
  two a payload is, and never branches on the vocabulary.
- Native, a by-reference payload may be shared memory; in the browser it is
  whatever the transport can do. **The correctness of the design does not depend
  on which**, and a test proves the two paths produce the same document.
- `SamplesDomain` stops being shaped differently from the others.

**Acceptance:** an edit over samples and an edit over points travel the same
code path in the application core; a by-reference payload and a by-value payload
of the same edit produce byte-identical results; the browser build compiles with
no shared-memory path at all.

*Risk, recorded because it is the weakest generalization here: this seam has one
implementor per branch today. If AP4 cannot be written without inventing a
second one, it waits - a trait designed against a single implementor is designed
wrong, and this plan says so about its own milestone.*

**Closed 2026-09-06 by finding it already true, and nothing was built.** The
milestone asked for "one seam that says which of the two a payload is, and never
branches on the vocabulary". That seam exists and is called `Domain`, and the
second half is already an invariant rather than an aspiration: `payload`,
`refusal`, `current`, `project`, `label` and `coalesce_key` are the whole of what
the core asks of a vocabulary, there are exactly four call sites (all in
`Editor`), and **no module of the application core names a vocabulary** - not
`editor`, not `application`, not `context`, not `echo`, and not `FormEditor`.
Grepping the five for `SAMPLES`/`POINTS`/`EVENTS`, `domain.name` and an
`isinstance` against a domain returns nothing.

So the two branches are not a distinction to be introduced; they are two
implementations of one interface, and there are three of them. **By value** -
the state crosses the ABI and the crate answers with the edit and its inverse -
is `points` and `events`, two implementors. **By reference** - the state lives
elsewhere, the client applies it and the inverse rides on the wire - is
`samples`, one. Adding a marker saying which is which would be machinery nothing
reads, which is exactly what this milestone's own risk note says to refuse.

**What the milestone's second half was really about is transport, and it is a
non-goal of this branch.** "Native, shared memory; in the browser, whatever the
transport can do" is about how bulk travels, not about how an edit is expressed:
a stroke's run crosses today as floats in the OSC event, and moving it to the
shared segment is a native fast path under an ownership rule that is already
decided. It belongs to the GUI track's data-path work, not here.

**One leftover, filed rather than fixed** (see "Found by use" below): the
temporal coupling in `SamplesDomain`.

### AP5 - The application core moves to Rust

Only now, and only what AP0-AP4 have already proven is common.

- Down: id derivation (already there from AP1), the diff, the acknowledgement
  protocol (`Echo`), the routing table, the undo/redo walk across registered
  editors, and the unit bridge (beats/seconds <-> timeline samples).
- Stays in each client: `Domain.project` - writing a payload onto the client's
  own objects, which is by definition the client's - and a `View.build` for a
  view a client defines.
- The catalogue views (waveform, `bpf`, pianoroll, the multitrack's lanes and
  clips) build through `tree.rs`, so the standalone host gets the same function
  from the same code rather than from a second implementation.

**Acceptance:** the same gestures applied from Python, from the web client and
from a standalone host produce the same document *and the same tree of widget
ids*; the `editing/` line count in both clients drops to the domains, the views
a client defines and the idiomatic surface; no numeric or timing rule is left
implemented twice.

**First slice landed 2026-09-06: the difference is the core's** — *and was
retired on 2026-09-07, see (4) below.* `clausters_core::guidiff` decided what to
send so a host drawing one picture draws another — the sets, or the word that
says the shape changed — bound as `clausters_gui_difference` (core ABI v40) and
`guiDifference`, both taking the two documents as JSON text and answering as
JSON text. Python's `publish` called it and the Python walk went; the web client
bound it and owed only the caller.

It went first because it was the piece written in **one** language and about to
be ported into two — lowering it prevented a divergence rather than repairing
one, and that was the right call on the day even though the module has since
gone: it stopped the walk from being written a second time in TypeScript, which
would then have had to be deleted twice. What replaced it is not a better
difference but the discovery that **no difference can be computed at all**
outside the host, which is what the rest of this milestone is about.

**Two of the things the milestone listed were already single, and the audit is
the deliverable rather than the move.** The **unit bridge** is four one-line
compositions per client over calls that are already the core's
(`tempo_map.secs_at` -> `secs_to_samples`, and their inverses) — read side by
side, Python and TypeScript compose the same core calls in the same order, so
there is no second rule to remove and lowering them would add an ABI call per
conversion per widget per draw. The **echo's** staleness test is one comparison
(`against != 0 && against < floor`), identical in both. Neither is a rule
written twice; both are the same rule called twice, which is what a binding is
for.

**What AP5 turned out to be about: the picture has one owner.**

The difference landed and the flicker went, and then a day of use turned up a
row of defects that all sit under one premise nobody had written down.
`Application.publish` computes the difference between the tree it has just
derived and `_published`, and sends the result to the host's tree. **That is
correct only if `_published` equals what the host holds.** It does not, and it
cannot:

- **The host mutates on its own.** `reparent_clip` moves a clip between lanes
  while the hand is still holding it, the drag writes an offset per frame,
  `set_y_view` and `scroll_set_view` write a lane's window, and the selection
  and the header controls write too. Some of those report at the release; the
  ones that are **screen state report nothing, correctly**, because screen state
  is the host's by the four-layer rule. So the equality the difference depends on
  is not merely lost sometimes — it is not reachable.
- **A `/gui_def` can fail in silence.** `define_node` answers a widget it cannot
  build with `warn!` and nothing else, and the protocol has no reply to a def.
  The client records the tree in `_published` as though it had landed, and every
  later difference is computed against a picture the host never built.

**Versioning the picture was tried on paper and does not work.** If the host
stamps a picture generation and the client names the one it computed against, a
disagreement leaves the client redefining whole — the flicker this branch
removed, now on **every** drag, since the host mutates every frame. Narrowing it
means the host reporting its mutations and the client applying them to
`_published`, which is the client modelling the host's rules in two languages:
precisely what the non-divergence rule forbids, and the kind of drift no compiler
finds. Every option that keeps a picture on the client ends there.

**So the client stops keeping one.** It sends the tree; the host compares it with
what it holds — the only copy that is true — and decides what to rebuild. This is
the convergence with the DOM that the design conversation was reaching for, and
it is **reconciliation, not addressing**: paths were considered and rejected,
since a path encodes a position and re-parenting a clip is the multitrack's most
common gesture, so a path would go stale exactly where a derived id does not.

`/gui_def` stops meaning *free this and build that* and comes to mean *make it
look like this*. The host, which is the only thing that knows which widget kept
its identity, is what decides what survives — and that kills the complaint this
branch opened with at the root: **the zoom is not restored, it is never
destroyed.** Today the client has to guess a redefine narrow enough to spare a
state it cannot see.

The division that falls out of it, and the reason it belongs to this milestone
rather than to the protocol: **the client says what it redrew; the host decides
what that costs.** The client decides both today, and the second is a drawing
decision it was never meant to have — choosing how much of the screen is rebuilt
is drawing.

**AP1 is what makes it possible, and it has already landed.** Reconciling means
matching an old child to a new one by identity, which is exactly the id derived
from `(structure, role, key)`. The branch's first milestone turns out to be the
enabler for the fix to the branch's worst problem; that is not how it was
planned, and it is why this is written here rather than re-derived later.

**What it costs, unpainted.**

1. **Bandwidth.** A tree per redraw instead of a delta. Mitigated by publishing
   the smallest subtree the client knows it touched rather than the window.

   **Measured 2026-09-07, and the mitigation is not optional — it is the whole
   answer.** One clip dragged for 60 frames over four sizes of multitrack, each
   frame published through the client's own difference, weighing what goes out
   three ways (`tests/test_gui_publish.py`, which asserts the shape of this):

   | piece | today's delta | the touched clip | the whole window |
   |---|---|---|---|
   | 4 lanes x 4 clips | 21 B/frame | 91 B | 1.9 kB |
   | 8 x 16 | 21 B | 91 B | 13 kB |
   | 24 x 40 | 21 B | 91 B | 97 kB |
   | 64 x 100 | 21 B | 91 B | 652 kB |

   Three readings, and the third is the decision. The **delta is flat** — the
   difference finds the one prop that moved, so a drag is one `/gui_set` however
   large the piece is. The **window is not affordable**: at 60 Hz the largest
   here is 39 MB/s of JSON, and it grows with the piece rather than with the
   gesture, which is the wrong shape whatever the constant. And the **touched
   subtree is flat too, at four times the delta** — 5.5 kB/s, which is nothing.

   So `_published` can go, and the granularity is the condition rather than a
   refinement: the client must publish **the widget its edit named**, which it
   knows because an intent names a node and `AP1` derives the widget id from it.
   Publishing the window and letting the host reconcile would be correct and
   unusable.

   **`_published` went on 2026-09-07**, and the condition is kept by making the
   granularity the **caller's** argument rather than a rule inside `publish`:
   `publish(widget, tree, window=…)` names any widget, so an editor publishes
   what its edit touched. Nothing enforces it, and nothing can — which widget an
   edit named is knowledge only the editor has.
2. **Which props are the host's and survive a reconcile.** Implicit today in "a
   redefine destroys everything"; it has to become explicit, per widget kind —
   zoom, scroll, selection, focus, expanded, the overlays for what is in flight.
   **This is the part to expect to have underestimated**, and it is the real
   work. Keeping too much is worse than today's defect: a zoom that survives a
   lane which is no longer the same lane.
3. **The blobs.** A tree with blobs goes whole today because a `/gui_set` carries
   no blob index. Reconciling, a widget whose blob did not change must not have
   to re-send it, so there has to be a way to say *keep the one you have*. For
   the editor-grade waveform that is not a detail.

   **Landed 2026-09-07, and the word is `"data": "keep"`.** It is a value of the
   prop that already names the samples rather than a sixth carrier beside
   `data`/`blob`/`buffer`/`path`/`cache` — `blob` is how `data` travels, so what
   is being deferred is one thing however it arrived, and a client that spilled
   to a blob and one that inlined a short run say the same word. The reconcile
   carries the run **and the resolved pyramid**, both behind an `Arc`, from the
   widget that survived onto the widget that replaced it, so honouring a keep is
   two refcount bumps against re-sending minutes of audio.

   **Asked for rather than inferred from silence**, which was the choice worth
   making: silence already means something else here — a clip that states no
   source has no take body at all, which is how a roll-only clip is spelled — so
   a keep has to *say* the body is there before it can say what fills it. And a
   keep the host cannot honour (a widget that is new, an id that now names
   another kind) is **reported**, because an empty waveform looks exactly like a
   waveform of silence: the one failure this word can produce is the one a
   reader cannot see.

   Both clients spell it `KEEP`, one constant each, and a parity vector holds
   them to it — the sweep crosses every builder with every option and would not
   have caught this, since it is a new **value** of an option that already
   exists.
4. `guidiff` moves into the host — a move rather than a rewrite, its tests with
   it — and the FFI and wasm exports go. **Both clients lose surface**, which for
   the non-divergence rule is the right direction, and the ABI counter moves for
   it.

   **Done 2026-09-07, and it turned out to be a retirement rather than a move**
   (core ABI v42). The exports are gone, both clients lost the surface, the
   counter moved — every outcome this item asked for — but the module did not
   land in the host, because there is nowhere in the host for it to land.

   **Why, and it is the same argument as the one above, one level down.** A
   difference is a comparison of two *documents*, and it is only correct when
   one of them equals the picture on screen. Inside the host that copy is no
   more reachable than it was in the client: the host would have to keep the
   last `GuiNode` it was handed, and it writes to its **widgets** without
   writing to that — a drag writes an offset per frame, a wheel writes a
   window — so a redraw restating the offset the document already carried would
   diff as *unchanged* and leave the widget where the hand left it. That is
   `_published`'s defect exactly, reproduced one process over.

   What the host has instead is the comparison that is always true: the document
   it was handed against **the widget tree it draws**. That is `reconcile`, it
   already landed with `O23`(b), and it is not this module in a new home — the
   two sides are not the same kind of thing. So `guidiff` had no caller left
   anywhere, and a module in the host that nothing calls is worse than one in
   the core that nothing calls.

   **What went with it**, and it is worth naming because it was good: the
   generated pass over two thousand pictures nobody chose, asserting that no set
   ever named a widget the redefine beside it removed. It tested an arithmetic
   that no longer runs. Its counterpart on the client side went the same way,
   and what replaces both is the reconcile's own suite, which asks the question
   against the widgets rather than against a document.

**What does not change.** The acknowledgement and the version stay as they are.
They are about the **document**, the document is the client's, and they are used
correctly (audited 2026-09-06; the three defects that audit found are in "Found
by use" and none of them is in the mechanism). The picture needs no version
because there comes to be one picture.

This was checked against prior art after the fact, not derived from it - see
"Design reference: what the field does, and where we differ" above, which names
the reconciliation as the field's standard answer and gives the track's-contents
question its forty-year-old structure (under our own name, `Lane`).

**What is not settled**: the list in (2). Without it a reconcile keeps too much
or too little, and there is no way to write the acceptance below until it exists.

**Settled 2026-09-07, and it is a structure rather than a list** (`O23`(a) in
`crates/clausters-document/PLAN.md`). `clausters_document::view` holds a `View`
per window - `visible`, `scroll`, `quant`, `autofit`, `selection`, `selected`,
`focused`, `detail` - with a `TrackView` and a `LaneView` looked up by id. It is
parallel to the model on Live's shape, a session carries a **list** of them
because two windows disagree on purpose, and `View::prune` makes *state goes when
the thing goes* a method rather than a habit. The list in (2) is now those
fields, so the acceptance below is writable and the reconcile is the only half
left. One written decision was refined rather than stepped over: AP3's *"nothing
about screen state reaches a history or a file"* still holds for the document and
the history, and no longer for the session file - which is not the document, and
is where this project's own framing of O23 asked the view to be saveable.

**Acceptance, for this half:** `Application._published` is gone; a client holds
no picture; the host answers a def by reconciling against what it draws; a lane
whose identity persists keeps its zoom, its scroll and its selection across any
edit anywhere else in the window, and that is true from Python, from the web
client and from a standalone host because it is one piece of code.

**What AP5 still owes, and why each is where it is.**

- **The routing table** and **the undo/redo walk** are both open, and both are
  in "Found by use" below with a checkbox — this list says what the milestone
  did not do, and a pending item filed only among the reasons for not doing it
  is a pending item that reads as closed.
- **The web client**, which has neither the `Application` this milestone is
  about nor the `GuiHost.redefine` its granularity is sent through. Both are in
  "Found by use" below and both are owned by **`W30`**
  (`clients/web/PLAN.md`), which is written to run *after* this branch — so the
  acceptance's "and that is true from the web client" is the one clause of it
  this branch does not deliver, deliberately and with a milestone naming when it
  will be.
- **The catalogue views.** Building `waveform`, `bpf`, `pianoroll` and the
  multitrack's lanes through `tree.rs` is what gives the standalone host the same
  function from the same code, and it is entangled with AP6's convergence: the
  lanes and clips are the view that has to be named first. It lands there.

  **It has had no home since AP6 was closed by removal** *(noticed 2026-09-07,
  reading this milestone against the status)*: "it lands there" now points at a
  void milestone, so this is an item with a name, an argument and nowhere to be
  done. It goes to `O24`, whose three applications are what name the lanes and
  clips — and it is written down here rather than moved in passing, because
  which milestone owns it is `O24`'s decision to record and not this file's.
- **The reconciliation itself** — everything the section above describes — is
  the milestone's remaining half. **The host's side landed 2026-09-07**
  (`widget::reconcile`, `O23`(b)): a `/gui_def` over a tree the host draws is
  matched widget to widget — by id anywhere in the tree, by position for the
  bodies the wire does not address, and only where the wire's own type string
  agrees — and the host's own state is carried across. The def still wins on any
  key it states, which is the wire's *nothing said is nothing written* and is
  what keeps a script able to drive a view at all.

  **What is left is the client's side.** Cost (1) above is now measured, and it
  answers with a condition rather than a yes: `_published` can go **when the
  client publishes the widget its edit named** rather than the window — flat in
  the size of the piece, four times today's delta — and not otherwise, because
  publishing the window is 39 MB/s at drag rates on a large piece. So the
  remaining work is a granularity change in `editing/`: `publish` comes to take
  the subtree an edit touched, and the picture goes with it.

  **And it waits on a caller, which is an ordering finding rather than an
  excuse** *(found 2026-09-07, doing the work)*. `Application.publish` has **no
  caller in the package**: `FormEditor` was the only thing that ever made a
  publish happen, it was deleted on 2026-09-06, and what an `Editor` uses today
  is `correct(widget_id, **props)` — narrow, per widget, already the granularity
  the measurement endorses. Grep says the entire remaining call graph is the
  tests'. So rewriting `publish` now would be designing a seam against **zero**
  implementors, which is what this plan refuses in `AP4`'s own risk note and for
  the same reason. It is written against the first editor that publishes again,
  and that editor is `O24`'s multitrack. The host's half does not wait on any of
  it: a redefine already stops destroying a zoom, whoever sends it.

  **It stopped waiting the same day, because (4) forced it** *(2026-09-07)*.
  Retiring `guidiff` takes `gui_difference` out of the C ABI and the wasm, so
  `publish` cannot compute a difference whether or not anybody calls it. What it
  became is **less** machinery rather than a seam designed for nobody: it sends
  the tree it was handed to `define`, or to `redefine` when a `window` is named,
  and `_published`, `published` and `forget_window` are gone with the picture.

  **The granularity did not become the client's, it became the caller's**, which
  is the shape the measurement endorsed and the one that does not need `O24` to
  exist to be right. `publish(widget, tree, window=…)` names any widget, because
  `/gui_def` does; an editor that knows which node its intent touched knows which
  widget to publish, and one that does not can still publish the window and pay
  for it. The plan's worry — designing a granularity mechanism against zero
  implementors — does not apply to *removing* one and handing the decision to
  whoever redraws.

  The measurement's third column went with the difference: there is no delta to
  weigh any more. The table above keeps it as the record of what it was, and
  `test_gui_publish.py` now weighs the two answers that are still reachable — the
  touched widget, flat in the size of the piece, and the window, which is not.
**The standing reason for this milestone**, said by the user on 2026-09-06 while
reading the day's defects: *the editing logic has to be in Rust so that it is in
one place and consistent across clients*. Every defect found by eye that day was
in a rule the **view** applies and only Python holds — the threshold that told a
move from a trim, the rule that collapses a simultaneous aggregate into one
layered clip, the decision of what counts as a change of shape. None of them is
about the arrangement's model, all of them decide what a hand sees, and each
would have to be written a second time for the web client and a third for the
standalone host. That is the argument, and it is recorded here so it is not
re-derived from the next defect.

### AP6 - `FormEditor` converges - **closed by removal, 2026-09-06**

**The milestone is void and its subject is deleted.** `FormEditor` was removed
that day - source, examples, tests and the book chapter - and `clausters.form`
was frozen as a small set of client-side data structures with no view. What
replaces it is not a converged `FormEditor` but a **session** in
`crates/clausters-document` (source, region, lane, track, automation) with
three classic applications over it; the design is that crate's `PLAN.md`, "The
turn: the arrangement stops being a projection", milestones `O21`-`O24`.

**Why converging it was the wrong target, said once so it is not re-attempted.**
This milestone assumed the multitrack's structure was a *projection* of a general
tree and that porting the projection onto a better seam would fix it. It would
not have. A multitrack's own state - which track a thing is on, its order within
the track, its placement and its identity - is authored, durable and undoable,
and a projection has nowhere to keep it, so it kept it in the widget tree. Every
defect this branch found follows from that, and no seam under the projection
reaches it.

**What survives is the measurement below**, which is what proved the id work and
which stands whatever draws the lanes.

- ✅ **The multitrack's widgets are named** *(AP2's acceptance, moved here on
  2026-09-06 with the measurement that forced it; done the same day)*. A lane, a
  clip, a patch, its workspace and the ruler stop taking leased ids and ask for
  one by name through `FormEditor._widget_id`. A clip's name is the
  **placement** it draws - its document node id (`Ox` O14) - which survives a
  redraw, a save and a reopen, so the widget keeps its number across all three.
  An element the document does not name yet takes a lease, which is the honest
  answer: it has no identity to be stable against.

  **What this settled is what `draw` may do**, not the naming. Asking for a node
  id derives the document, and the milestone was written expecting that to be
  the obstacle. It is not: the editor holds one (`Ox` O13), so the ordinary
  answer is a lookup, and it is re-derived only when the arrangement moved by a
  route no gesture took - which is exactly when the picture has to be rebuilt
  anyway. So the derivation is not a cost `draw` pays, it is one it schedules.

  **Measured, on a host with a real namespace.** Two draws of one composition
  now give the same lane ids (20001, 20003, 20004 twice, against 20005/20007 ->
  20009/20011 before); a redraw of the same piece costs **zero definitions and
  zero sets**; and dragging a clip in a piece of many, then redrawing, costs
  **zero definitions** and leaves the clip the same widget.

**Acceptance: withdrawn with the milestone.** It named `composer.py`, which is
deleted. **AP2's half of it moves to `O23`**, where it is the right shape rather
than a narrower redefine: editing one thing in a piece of many emits no
definition and no free, and a scroll position and a selection survive because
the host **reconciles** and never frees a widget whose identity persisted.

### AP7 - A second application, to prove the abstraction

An application that is **not** the multitrack: a small bundle of editable
subviews over structures with nothing composed behind them - a buffer, a curve
and a timeline in one window set with one undo order. It is written as an
example (`clients/python/examples/`), in both clients, because an abstraction
with one real consumer has not been tested.

**Acceptance:** it is written with the supported surface and nothing is added to
the clients to make it possible; the two versions are one program in two
languages; an undo walks the interleaving of what was done in each subview.

**Still wanted, and now cheaper to judge** *(2026-09-06)*. The multitrack is no
longer the abstraction's first consumer - `O24`'s three applications are - so
this milestone stops being "prove it against the one thing we have". Whether it
is still a milestone of its own or is absorbed by `O24`'s audio editor is
decided there.

### AP8 - The pass over the packages, and the plans keep what is worth keeping

The milestone that makes deleting this file legal.

- **Docs:** `docs/architecture.md` gains the application scope and its place in
  the four layers; `docs/gui-protocol.md` takes whatever the diff and the derived
  id change on the wire; the Python book's composition chapter and the web
  book's equivalent follow **- both of those are gone, deleted 2026-09-06 with
  `FormEditor`, and what replaces them is `form.md` in each book plus whatever
  `O24` writes** ; `docs/bindings.md` and the parity tests take every new
  symbol. `scripts/check-docs.sh` before committing anything a book reads.
- **Decisions:** `docs/decisions.md` records the two worth recording - the
  application as the unit that owns the id space and the screen state, and a
  widget id derived from a structure's identity rather than leased.
- **The plans keep the durable half of this file:** the new track in
  `clients/gui/PLAN.md` (the seam, the derived id, the diff, the screen state),
  a pointer in `clients/python/PLAN.md` and `clients/web/PLAN.md`, and - in
  `crates/clausters-document/PLAN.md` - the answer this branch gives to the open
  question about the second document. **That last one is done**: the question
  ("may one element be placed twice, and what does an intent name if it is?")
  closed on 2026-09-06 into `O21`'s **region**, which is the placement given an
  identity of its own.
- **Checks:** `cargo fmt`, `cargo clippy --all-targets` clean,
  `.claude/skills/feature-matrix/check.sh`, `npx pyright` in `clients/python`,
  `./build.sh && ./test.sh` in `clients/web` with the parity vectors regenerated,
  and the touched examples run by hand.

**Acceptance:** nothing in this file is the only copy of anything, and deleting
it loses no decision.

## What is already in Rust, and must be read again against the new design

**Written 2026-09-06, when the arrangement stopped being a projection.** The
turn deleted `FormEditor` — a Python/TypeScript driver — and it deleted nothing
in Rust. **Roughly 8000 lines of multitrack behaviour are already implemented in
the host**, plus the whole document crate under them, and none of it was
reviewed against a design that did not exist when it was written. This section
exists so that is not mistaken either for "the multitrack has to be built from
nothing" or for "the Rust half is fine and only the clients changed". Neither is
true.

| Where | Lines | What it already does |
|---|---|---|
| `host/placement.rs` | 473 | **one geometry for every box on a time axis** — a note in a roll and a clip on a lane are the same span, grabbed by the same three parts, snapped by the same grid, moved as a block, quantized, hit-tested in a rect |
| `host/graphics/track.rs` | 1702 | the lane and its clips as drawn: the body, the header, the grips, the three clip bodies (waveform, roll, curve) |
| `host/gestures/nav.rs` | 1003 | the lane `Stack` and its bands, `reparent_clip`, edge-scroll while dragging, the vertical view |
| `host/interact/*` | 1071 | the hit-tests, the drag arithmetic, and every edit-back payload a lane or a clip emits |
| `host/ruler.rs` | 2267 | the beats/bars/seconds rulers and the tempo map they read |
| `host/layers.rs`, `scroll.rs`, `play.rs` | 963 | the edit layer of a layered clip, the shared time axis, the playhead |
| `crates/clausters-document` | 5861 | the document, the intent vocabulary and its one applier, the log and its inverses, the typed selection and clipboard, the session format |

**What that means for `O21`-`O24`.** The behaviour is not the problem and mostly
survives: `placement.rs`'s own module doc already says a lane and a semitone row
are one structure, which is the observation the whole turn rests on. What has to
be read again is **what each of those files takes as its input**, because that
is what changes:

- **The structure is the widget tree, everywhere.** `reparent_clip` moves a
  `Widget` between two `children` vectors; `clips_event_args` walks a lane's
  children; `Stack` is built from widget ids at press time. Under a session those
  become list operations on a **lane**, and the widget tree becomes something
  derived. That is the same code doing the same arithmetic against a different
  owner — a real change, and not a rewrite.
- **`WidgetKind::Clip` is three numbers and a label**, built from a `field` node
  with an `offset`. A **region** is more (its own identity, a source reference,
  fades, gain, a layer) and the extra fields have to come from somewhere the
  host holds rather than from a GuiDef prop per redraw.
- **Nothing in the host holds a track's identity across a redefine** except the
  derived id `AP1` gave it. `O23`'s reconcile is what turns that from a
  convenience into the mechanism.
- **The only document that ever existed is `form`'s.** `Body`'s five variants
  are `form`'s five primitives given a serde form, and the only door into the
  crate from either client is `form/document.py` / `form/document.ts`. So the
  module just relegated is the one every writer goes through - including the
  standalone host's own save/reopen loop, which has nothing to do with `form`.
  `O21` names this and has to move the door; it is written up there rather than
  here because it is that milestone's work and not this branch's.
- **The document crate is complete for the premise it was written under**
  (`O1`-`O20` all closed) and its premise is the one that moved. Its types are
  not deleted — `O21` says they become what a region may contain — but every
  milestone that read "the tree" has to be re-read as "the session, whose
  regions may hold a tree".

**The rule to keep while doing it**: the host's multitrack code is the most
exercised, most eye-tested part of this project, and it is the half that was
right. When something has to change there, the question is what its **input** is,
not whether the behaviour was correct. Rewriting the arithmetic because the
owner moved is how a working editor becomes a new set of defects.

## Found by use

- ⬜ **`GuiHost.redefine` exists in Python and not in the web client** *(found
  2026-09-07, writing the wire's word for keeping a widget's bulk)*. The narrow
  redefinition — a `/gui_def` of a widget **inside** an open window — is the
  channel this whole branch is about, and only one client has a door for it.
  `clausters.gui.host.GuiHost.redefine(id, tree, window=…)` sends the message
  and does the bookkeeping a *part* needs: the names under the old subtree go,
  the new ones join what the window already had, and the handle a script holds
  stays the one it holds. The web client has `define` and nothing else, and
  `define` is written for a **window** — it replaces the handle's whole name map
  with the names of the tree it was handed, so calling it on a subtree leaves
  the window resolving only that subtree's names.
  **What makes it worth an entry rather than a port on the spot**: the message
  goes out either way, so this is not a page that cannot redraw a lane — it is a
  page whose *names* stop resolving when it does, which is silent, and shows up
  as a handler that stopped firing rather than as an error. It is the standing
  rule's own case (a verb in one client and not the other), it is the door
  `data: "keep"` is sent through, and it is the same shape as `Application`
  below: the machinery has one implementation because the one caller that
  exercised it was Python's.

  **Owned by `W30`** *(`clients/web/PLAN.md`, written 2026-09-07)*, together
  with `Application` below — two halves of one gap, ported together. The entry
  stays open **here** because this is where it was found and this is the branch
  it is read against; what it is not is this branch's work, and `W30` says why:
  it runs after `AP5`'s remainder, `AP7`, `AP8` and `O24`, because porting a
  seam that is still moving is porting it twice.

- ⬜ **The reconcile has no example to see it in** *(found 2026-09-07, going to
  check it by eye)*. `widget::reconcile` has seven unit tests and **no manual
  test surface**: no example in either client sends a second `/gui_def` over an
  open window. `multitrack.py` — the one that looks like it would — drives
  everything through `/gui_set`, and grep finds no `redefine` in any example in
  the tree. It is the same absence as `Application.publish` having no caller,
  seen from the other side: `FormEditor` was what made a def happen twice, and
  it went. So the behaviour that fixes the branch's opening complaint cannot be
  watched happening, which for this project is half a check. It closes with the
  editor that publishes again (`O24`'s multitrack), and that example is where
  the by-eye pass belongs — a lane zoomed in, a clip added to another lane, and
  the zoom still there.

- ✅ **A change of shape redefined the whole window, so an edit in one lane
  cost every other lane its screen state** *(found 2026-09-06 by the user, by
  eye, in `composer.py`: splitting a clip works and the **vertical zoom of every
  lane** goes back to where it started; the same on **moving a clip to another
  lane**, which is the case a hand meets first — a drag **within** a lane costs
  nothing, which is what says the difference is doing its job)*. `Application.publish`
  has two answers, the difference and `/gui_def` on the window, and a widget
  that appeared can only arrive by the second — there is no insert on the wire.
  But the shape changed in **one lane**, and redefining the window frees and
  rebuilds every other one, taking the screen state the host held for each with
  it.

  **The wire already allows the narrow answer**: `/gui_def <id> <json>`
  redefines *any* widget, not only a window ("re-sending an existing id
  redefines it"), so the fix is to walk down to the smallest subtree whose shape
  moved and redefine that. What it needs is on this side: `GuiHost.define`
  collects the names of the tree it is handed and replaces the **window
  handle's** whole name map with them, so redefining a subtree through it today
  would leave the window resolving only that subtree's names. So the work is
  host-client bookkeeping — a define that merges a subtree's names into the
  handle instead of replacing them, and frees only that subtree's ids — and it
  was left out of AP2 deliberately rather than missed.

  **It was not an accepted boundary** *(the user, 2026-09-06: it is
  unacceptable, and the multitrack view's implementation changes for it)*. "A
  prop change costs nothing and a structural change costs the window" was better
  than what it replaced and still wrong at the first gesture a hand makes.

  **Fixed the same day, in the core.** `guidiff` answers `{whole, redefine,
  sets}` instead of "the whole tree or nothing": it walks down to the smallest
  subtree whose shape moved and names *that* widget, and a node that cannot be
  patched is redefined by its **parent** rather than by the window. The window
  goes whole only when the root's own shape moved, which is the one case with no
  parent to name. `GuiHost.redefine` is the bookkeeping a part needs — the names
  under the old subtree go, the new ones **merge** into what the window already
  had, and the handle a script is holding stays the one it holds — which is what
  `define` could not do, since it replaces the handle's whole name map.

  Measured: moving a clip between two lanes now sends **two** `/gui_def`s, one
  per lane it crossed, and **zero** window rebuilds. Every other lane keeps its
  zoom, its scroll and its selection. The ABI counter moved for it (v41): the
  same symbol answers a different document, so a staged library one version
  behind would have been read as "nothing changed" on every redraw.

  **And the host had to learn to receive it** *(found the same day by the user,
  by eye: the zoom survived and a **split did not show** — the client sent the
  lane's new tree and the picture did not change)*. The wire has always said a
  def names any id, and the host only ever built a **renderable** document for a
  `window`: a def of anything else was recorded in the registry, logged, and
  invisible. So the narrow redefine was, for one commit, a message nobody drew —
  and the zoom "surviving" was partly nothing being redrawn at all. `define_node`
  now splices a non-window def into the typed tree the front draws, at the widget
  it names, and answers `Redraw` rather than `OpenWindow`. The window this widget
  belongs to is read **before** the registry re-roots it, which is the one thing
  the fix turns on.

  Worth keeping as the lesson: the client half measured perfectly — two defs,
  zero window rebuilds — and proved nothing about what a person sees. What
  caught it was an eye on the window.

  Everything this turns up is recorded **here**, in this file, because all of it
  is to be dealt with — including by changing the design.

- ✅ **A set is addressed to a widget the redefine beside it just removed**
  *(found 2026-09-06 in the log, with the narrow redefine running: the host
  warns `/gui_set 1005: no such widget` immediately after
  `publish ... 1 redefine(s) [1002] and 2 set(s): 1003(offset), 1005(offset)`)*.
  The two halves of one publish disagree: a widget is redefined whole **and** a
  set is sent for something that was inside it, so the set lands on a number the
  host has already freed. `guidiff::walk` is written to make that impossible — a
  node that cannot be patched contributes nothing but its own id, and its
  `below` is discarded — so either that reasoning has a hole or the ids in the
  two halves come from different pictures. **The first thing to do is not to
  read the walk again but to catch it**: assert, in the core's tests, that no
  set names a widget under any redefined id, over a generated pair of trees.
  This is very likely the cause of "everything started failing at once", since a
  set to a freed id leaves the client believing it drew something it did not.

  **A candidate cause arrived later the same day, from AP5's audit, and it is
  the third possibility rather than one of the two above**: the walk may be
  correct *and* the two halves may come from one picture - and the set still
  land on nothing, because the picture both halves came from is the **client's**
  (`Application._published`) and the widget was freed in the **host's**. `1005`
  would then be present in both trees the difference compared, which is exactly
  why a set was emitted for it, and absent only where it mattered. See "What AP5
  turned out to be about: the picture has one owner" - the premise that the two
  are equal is not merely violated here, it is unreachable.

  **It does not change the first thing to do, it sharpens what the answer
  means.** Write the generated-trees assertion anyway: if it **passes**, the
  walk is exonerated and this entry is evidence for AP5's premise rather than a
  bug of its own; if it fails, there is a hole in the walk to fix regardless of
  what AP5 later does to the picture. Either way the test is cheap and answers a
  question that is currently open. What must not happen is this entry being
  closed by assumption because AP5 has an explanation that fits.

  **Both tests are written, and the entry stays open** *(2026-09-06)*. The
  arithmetic's is `guidiff::no_set_ever_names_a_widget_the_redefine_beside_it_removed`
  - 2000 generated pairs of pictures, a deterministic generator so a failure
  reproduces from its seed, asserting that no set names a widget under any
  redefined id (the redefined widget itself included), that no redefine sits
  inside another, and that both halves address widgets **both** pictures hold.
  It counts what it generated and fails if the run never produced a publish
  carrying a redefine and a set at once, which is the one case it exists for.
  The wire's is `clients/python/tests/test_gui_publish.py`, whose host double is
  a **registry** rather than a recorder: a `/gui_def` of a subtree frees every id
  under it and builds the new one, and a `/gui_set` to a number the host does not
  hold raises where the real host only warns.

  **Both pass, so the walk is exonerated and this entry is now evidence for
  AP5's premise.** What a pass means is bounded and the test file says so: the
  publish loop is sound *while the client's picture and the host's agree*, which
  the double makes true by construction. The running program is where they do
  not, and that is the third cause. The entry closes when the host stops being
  the only one who knows what it is drawing - which is `O23` - and not before.

  **Closed 2026-09-07, by removing the sentence it was about.** A publish is a
  `/gui_def` and nothing else: there is no set beside a redefine, so there are
  no two halves to disagree. The third cause is gone with them — no client holds
  a picture to diff against, so no message is addressed from one. Both generated
  tests went with the arithmetic they tested; what asks the question now is the
  reconcile's own suite, against the widgets the host holds rather than against
  a document nobody is drawing.

  It stays here as the record of the diagnosis, which is the part worth keeping:
  the walk was right, the wire test was right, and the defect was in the
  **premise both of them were written under**. Two green tests exonerating a
  mechanism that was failing in front of the user is what this list is for.

- ⬜ **The window closes and the process spins at 100% CPU** *(found 2026-09-06
  by the user, twice; one earlier instance was a `composer.py` still running 55
  minutes after its window was gone, at ~19% of a core)*. Nothing is known about
  it yet beyond the shape: the window goes and the process does not. Two places
  to look, and neither has been: the front's loop after a window is dropped
  (`gui/app.rs`, `drop_window`) — a redraw requested for a window that is gone
  would spin — and the client's own wait (`Application.wait` /
  `GuiHost._wait_while`), which holds a thread until the window closes and may
  never learn that it did. It matters more than it looks: it is the failure that
  ends a session, and it leaves a stale host holding the port, which then makes
  the *next* run fail for an unrelated reason.

- ⬜ **A clip dragged past the first or last lane oscillates back to the start
  of the track** *(found 2026-09-06 by the user, by eye, twice)*. Holding a
  vertical drag against the top or bottom of the stack makes the clip jump to
  the beginning and back, repeatedly, while the hand is still down. **No event
  reaches the client for those frames** — the log carries none — so it is the
  host's own drag, not an edit being refused. Two things in `gestures` are worth
  reading together for it: the edge auto-scroll (`drag.rs::tick`) pans the
  group's window and re-applies the drag against the window it left behind, and
  `apply_clip_drag` maps the cursor through the group's **current** window. What
  has not been checked is what `LaneStack::at` answers past the ends of the
  stack, and whether the pan fires from a vertical overshoot at all.

- ⬜ **The playhead draws behind the clips** *(found 2026-09-06 by the user, by
  eye, after a lane was redefined in place)*. Not missing — behind. The line is
  a lane prop the transport sets (`playhead_at`), read from `_playline` on each
  use so a redraw's new widgets get it, so the client's half survives a
  redefine; what has not been read is the host's paint order for a lane whose
  subtree was spliced. It appeared with the narrow redefine, which is the first
  thing that ever rebuilt a widget *inside* an open window.

- ⬜ **A clip cannot be moved between lanes once a split has happened**
  *(found 2026-09-06 by the user, by eye: the split now draws — the previous
  defect — and after it, dragging a clip to another lane does nothing)*. The
  session's log carries exactly **one** `'lane'` event in the whole run, so the
  host stops emitting them rather than the client refusing them. What changed
  under it is that a lane is now rebuilt in place, so the suspicion is the
  gesture state a lane carries across a redefine — `LaneStack` is read at the
  press, and `reparent_clip` names a lane id the stack captured. Whether the
  set-to-a-freed-id defect above is the same bug wearing another face has not
  been checked, and should be first.

- ⬜ **Dropping a clip where another one already sits makes the lane draw as
  one layered clip, so both appear to vanish into one** *(found 2026-09-06 by
  the user, by eye — "the curve's clip moved by itself back to where it was" —
  and reduced to two lanes and one drag)*. The mapping rule says a **concrete**
  aggregate whose members are `SIMULTANEOUS` is *one thing on the timeline*, so
  it draws as one clip with layered bodies rather than a lane of clips
  (`_lanes_for`). That is right for a piece an author wrote that way — a voice
  and the envelope over it drag as one — and it is a surprise as the outcome of
  a **drag**: drop a clip on a lane at the offset the clip already there has,
  the destination's two members are now simultaneous, the threshold
  (`len(element) > 1`) is crossed, and the lane redraws as a single summary clip
  carrying both.

  Reproduced with two lanes of one clip each, both at offset 0 and the same
  length: after the `"lane"` event the model reads `audio [(0.0 take), (0.0
  other)]` and `other []` — exactly right — and the picture reads one clip
  labelled with the *lane's* name.

  **It is not a defect of the rule but of where the rule is applied**: the rule
  says what a composition *is*, and it is being asked what a gesture
  *produced*. It is filed rather than fixed because the choice — refuse the
  drop, offset it, expand the destination, or keep the collapse and say so — is
  the multitrack's design and belongs with AP6.

- ✅ **Screen state was keyed by an address, so a new thing inherited a freed
  one's** *(found 2026-09-06 auditing AP3's premise; fixed the same day)*. Four
  tables kept screen state under `id(object)`: a curve's held axis and its span
  (`PointsView`), a patch's box placements and which elements are expanded
  (`FormEditor`), and — added by AP1 the same session — an application's
  per-drawer id table. CPython reuses an address the moment an object is freed,
  **196 times out of 200** in a straight loop, so each of them hands its state
  to whatever lands there next. Reproduced end to end: expand an aggregate, let
  it go, make another, and the new one draws expanded. **Fixed** by keying on the
  object, weakly — the guard the tree already used one level down, where
  `Editing._structures` keeps a structure beside its number "so its `id` cannot
  be reused". Weak keys also let the state go when the thing goes, which is what
  screen state should do. What made it findable was asking what "keyed by the
  derived key" meant literally; what made it invisible is that every one of the
  four reads correctly and fails only after a free.

- ✅ **An `Application` built with no version callable read its editors before
  it had any** *(found 2026-09-06 writing AP2's tests; fixed the same day)*.
  `__init__` created the `Echo` — which asks for the version immediately — before
  assigning `self._editors`, and the version is read *through* that list when no
  callable was given. Every editor passes one, which is why 935 tests never
  touched it and why it surfaced only when a test constructed an application on
  its own. **Fixed** by making the list first. Worth keeping because it is the
  ordinary shape of a constructor defect: the object was correct for every
  caller that existed, and the seam had just been widened to admit one that did
  not.

- ⬜ **The routing table's tag list is written twice** *(found 2026-09-06,
  auditing what AP5 had left)*. `NOT_AN_EDIT` — which event tags are screen
  state rather than edits — is eight strings duplicated verbatim in
  `clients/python/clausters/gui/editing/editor.py` and
  `clients/web/src/gui/editing/editor.ts`. **The two agree today**, read side by
  side and checked; what makes it worth writing down is that nothing keeps them
  agreeing, and the failure is quiet: a tag one client treats as screen state
  and the other hands to a domain is a gesture that reaches a vocabulary which
  does not know it, answers nothing, and looks like a widget that does nothing.
  It was not done with AP5 because lowering it is a core module and two ABI
  symbols to hold one list, which is the machinery AP4 refused on the same
  grounds — so what this entry is waiting for is either a second reason to open
  such a module (a second GUI vocabulary constant that has to be shared) or a
  cheaper place to put it, and it should be settled *there* rather than by
  whichever milestone next reads the list.

- ⬜ **`Application` exists in Python and not in the web client, so half the
  publish machinery has one implementation** *(found 2026-09-06, writing the
  publish test)*. `clausters.gui.editing.Application` holds the picture per
  window, publishes the difference, forgets a window that closed, hands a
  host-less draw its ids and walks the pile round the registered editors. The
  web client has **no `Application` at all** - `clients/web/src/gui/editing/`
  has context, domain, echo, edit, editor, events, points, samples and view, and
  no application - and it has never had one: `git log -S"class Application"` over
  `clients/web/src` returns nothing. The core function is bound and exported
  there (`guiDifference`, `src/base/core.ts`) and **called by nobody**.
  *(That binding is gone since 2026-09-07 — the difference was retired with the
  reconcile — which makes the port cheaper rather than smaller: what is missing
  is the class, and a publish is now one `/gui_def`.)*
  Measured, the module's surface differs by `Application`, `ATTR`, `BASE_ID` and
  `watch` in Python's favour; what runs the other way is type aliases and two
  internals TS exports and Python keeps private (`contexts` behind `Editing.of`,
  `resolveEditorHost`), which is idiom rather than surface.
  **Why it was not visible until now**: the one override of `restructure` that
  made a publish happen lived in `FormEditor`, so both base editors answer
  `False` and the loop had no user left to diverge over. The next application
  gives it one. This is the standing rule's own case - a class that exists in
  one client and not the other - and it is why the wire test above has a Python
  half and no TypeScript twin.

  **Owned by `W30`** *(`clients/web/PLAN.md`, written 2026-09-07)*, which is
  where the port, its ordering and its acceptance now live — with `redefine`
  above, and with the second half this gap argues for: **the two clients read
  against each other, verb by verb, and the example directories side by side**.
  That reading is the milestone's own work and not a courtesy at its end,
  because of how this entry was found: the class went missing on 2026-09-06 and
  nothing failed, no test went red, and it surfaced a day later only because
  somebody writing a wire word needed the door to send it through. A gap that
  costs nothing to have is a gap nothing will find.

- ⬜ **The undo/redo walk is still each client's, and lowering it is a design
  step rather than a move** *(found 2026-09-06, scoping AP5)*.
  `Application.step` asks the editing context for a step's legs and hands them
  round the registered editors, each projecting the ones it owns through its own
  domain. Every part of that orchestrates **client objects**, so it cannot be
  lowered the way the difference was *(and the difference has since been retired
  rather than lowered further, which changes nothing here — this walk is still
  written twice)*: the crate would have to drive the clients
  rather than answer them, which is an inversion of control and a decision about
  what a binding may call back into — not a function to move. It is the last
  thing in the application core that is written twice once AP6 has taken the
  views, so it wants a milestone of its own and does not have one.

- ⬜ **`SamplesDomain` smuggles the inverse between two calls that do not mention
  it** *(found 2026-09-06, auditing AP4's premise)*. The crate's `samples`
  vocabulary has no field for "what this replaced", so the previous run - which
  arrives on the wire in the same event - waits in `self._previous` between
  `payload` and `current`. Two things follow. The interface does not say that
  `current` must be called right after `payload` and exactly once, so the
  contract lives in the flow rather than in the type; and a domain instance is
  therefore single-gesture, which nothing declares. It is **not a live defect**:
  there is one domain per editor, the four call sites run on one thread, and the
  only path that reaches `payload` also reaches `current`. It is filed because
  the coupling is invisible at the seam every other vocabulary is written
  against, and the fix - carrying the previous run in the payload the wire
  already put it in - has to answer what an extra field does to the coalesce key
  and to what the log records, which is more than a rename.

- ✅ **A publish that redefines widgets does not always tell the host what
  version it is now drawing** — *moot 2026-09-06 with `FormEditor`'s removal, and
  the observation kept.* The one offending site was `FormEditor._adopt_map` and
  it is gone; the single publishing site left (`Editor.open`) pairs the two. What
  survives is the reason it was worth an entry rather than a one-line fix: **the
  pairing is a convention and not a mechanism.** `publish` already answers
  whether it redefined anything, so the announcement belongs where the
  redefinition is decided. `O23` rebuilds that path and should close it by
  construction rather than by remembering. *(found 2026-09-06, auditing whether the multitrack
  uses the acknowledgement protocol correctly)*.
  `FormEditor._adopt_map` is the one publishing site of five that is not followed
  by `_announce()` — `open`, `load`, `update` and the editor's own `open` all
  pair the two. A tempo map that changed redraws the multitrack, the host frees
  the widgets the redefine names, and nothing retires the `Pending` entries in
  its `Outbox` that name them.

  **It is worse since AP1, not better.** An id is now derived from what it draws,
  so the rebuilt lane gets **the same id back** — which is the point of AP1 and
  is also what turns a stale pending entry into a live wrong answer:
  `Outbox::is_pending(def_id, widget_id)` reports an edit in flight on a widget
  that was built a moment ago, and nothing ever clears it, because the stamp that
  would retire it was never sent. A front that draws a pending value differently
  from an owner's draws that lane wrong from then on.

  **The first thing to do** is not to add the missing call — that is one line —
  but to ask why the pairing is a convention rather than a mechanism: `publish`
  knows it redefined something (it answers exactly that), so the announcement
  belongs where the redefinition is decided rather than at each of five call
  sites, four of which happen to remember.

- ✅ **`Echo.raise_floor` is called by nothing** *(found 2026-09-06, same audit;
  narrowed the same day)*. The floor rose at three places — `Editor.apply`,
  `FormEditor.rederive` and `FormEditor.load` — and all three assigned
  `self._floor = self._version` through the property rather than calling the
  method written for it. Two of the three went with `FormEditor`, so **one site
  is left** (`editor.py:461`) and the method is still dead. The documented verb
  and the mechanism in use are two spellings of one act, and it is now a
  one-line decision rather than a three-way one.

  What makes it worth an entry rather than a tidy-up is what it does to the
  tests. `Echo`'s own module doc says it is *"exercised by a test that never
  builds a structure, which is what a protocol should cost to check"* — and that
  test exercises a method no editor calls. The protocol is checked; the path the
  multitrack actually takes is not. Either the three sites call the verb, or the
  verb goes and the property carries the documentation — but the test has to end
  up on the road that is travelled.

  **Fixed 2026-09-07: the verb stays and the property goes**, in both clients.
  `Editor._raise_floor()` / `Editor.raiseFloor()` call it, beside `_announce`
  and `_correct` — where an *act* delegated to the `Echo` belongs — and the
  `_floor` / `floor` accessor pairs are deleted from `Editor` **and** from
  `Application`. Four accessors in Python, two in TypeScript, and none of them
  had a reader: the whole chain existed so one line could perform the act by
  hand.

  **Why that direction rather than the other.** A floor that can be assigned is
  not a floor. The act is *read the version, write it to the floor*, and the two
  halves of that can disagree — which is exactly the entry below, where a path
  meaning to reset the floor lowered it. One verb makes `stale` monotone by
  construction rather than by every caller remembering; the value pair made it a
  convention.

  **And the test moved onto the road.** The protocol test now raises the floor
  through the verb instead of assigning it, and a new case exercises the path an
  editor takes: a curve edited, undone, and then an edit-back naming the picture
  the undo replaced — refused, with the undo standing, and the next gesture
  against the picture that now holds applying. It lives in
  `test_gui_edit.py` / `gui-edit.test.ts` rather than in the generic editor's
  own suite, because that pair exists in **both** clients and the generic one
  does not. Both twins fail when the call is removed, which is what says they
  check the verb rather than the arithmetic around it.

- ✅ **`FormEditor.load` lowers the floor instead of raising it** *(found
  2026-09-06, same audit; **moot the same day** — `load` went with `FormEditor`,
  and no path left lowers the floor. Kept because the question it opened is
  still open and `O21` has to answer it: **what does a version mean across a
  load?** Whatever holds a session will have the same choice to make.)*. `load`
  set `self._floor = FIRST_VERSION`, and a floor that can go down is not a floor: everywhere else it only rises, which is what
  makes `Echo.stale` a monotone test rather than a race. Loading a composition
  whose version is below the one in hand leaves edits in flight from the piece
  that was just replaced applying **unchecked**, which is the exact case the
  floor exists to catch — the picture a gesture was made against is not merely
  older, it is a different composition.

  It is narrow: it needs a `load` with an edit in flight, and `load` is a session
  reopening. But the fix is not obviously `max(...)` either, because the two
  versions count different histories, and a counter from the previous piece is
  not a lower bound for this one. What the entry is waiting for is the answer to
  *what a version means across a load* — whether the context's counter is
  per-composition (in which case the floor should be the new context's version,
  not `FIRST_VERSION`) or per-session, which is a question about `Editing` and not
  about this line.

## The order, and why it is this one

1. **AP0 before anything**, because moving the seam in Python is cheap and
   moving it after AP5 means moving it in Rust and in TypeScript too.
2. **AP1 before AP2**, because a diff over leased ids is a diff over noise.
3. **AP1-AP4 before AP5**, and this is the load-bearing one: lowering the core
   before the seam exists freezes the wrong unit in Rust, with the id lease
   inside it.
4. **AP5 is two halves, and the second waits on a list rather than on a
   milestone.** The difference went down first because it was written in one
   language and about to be ported into two. The reconciliation - the client
   holding no picture at all - is ordered behind the one thing it cannot be
   specified without: **which props are the host's and survive a reconcile**,
   per widget kind. Writing that list is host knowledge and depends on no other
   milestone, so it can be done at any point before the work starts; starting
   the work without it means guessing what a reconcile keeps, and keeping too
   much is worse than the defect being fixed.
5. **AP6 after AP5**, because converging `FormEditor` onto a seam that is still
   moving is converging twice - and because the reconciliation changes the shape
   of every publishing path `FormEditor` has.
6. **AP7 after AP6**, because the second consumer is only evidence if the first
   one is already on the abstraction.
7. **AP8 last**, and the open question about the application document is
   answered there or explicitly left open - never decided in passing by a
   milestone that only needed somewhere to put a file.

## What would make this branch wrong

Written down so it can be checked rather than felt:

- If AP4 needs a second implementor invented to exist, the seam is speculative
  and waits.
- If AP5's move requires the document to read an opaque leaf, the projection is
  in the wrong place and the summary callback is what is missing.
- If AP7 cannot be written without adding surface to a client, the abstraction
  is not general and AP0's boundary is drawn wrong - not the example.
- If either client ends AP6 with a verb the other lacks, the branch has produced
  the divergence it was opened to remove.

## Status

- [x] AP0 - the seam, in Python only
- [x] AP1 - a widget id is named, not leased
- [x] AP2 - a redraw is a diff *(mechanism landed; acceptance met under AP6)*
- [x] AP3 - screen state is keyed by the thing, not by its address
- [x] AP4 - by value or by reference *(already true; nothing built, and why)*
- [~] AP5 - the application core moves to Rust *(the picture has one owner and it is the host: the reconcile landed both sides, `_published` is gone and the difference is retired. What is left is the web client, which has neither `Application` nor `redefine` - that is `W30`, and it is written to run after this branch - plus the routing table, the undo/redo walk and the catalogue views, in "Found by use" or owed to `O24`)*
- [x] AP6 - `FormEditor` converges *(closed by removal: the subject is deleted and the target was wrong; its measurement survives)*
      *(the Rust half was not deleted - see "What is already in Rust, and must be read again against the new design")*
- [ ] AP7 - a second application
- [ ] AP8 - the pass over the packages
