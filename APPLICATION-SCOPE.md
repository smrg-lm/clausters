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

**First slice landed 2026-09-06: the difference is the core's.**
`clausters_core::guidiff` decides what to send so a host drawing one picture
draws another — the sets, or the word that says the shape changed — bound as
`clausters_gui_difference` (core ABI v40) and `guiDifference`, both taking the
two documents as JSON text and answering as JSON text. Python's `publish` calls
it and the Python walk is gone; the web client binds it and owes only the
caller, which is AP0's seam. It went first because it was the piece written in
**one** language and about to be ported into two — lowering it prevented a
divergence rather than repairing one.

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

**What AP5 still owes, and why each is where it is.**

- **The routing table** and **the undo/redo walk** are both open, and both are
  in "Found by use" below with a checkbox — this list says what the milestone
  did not do, and a pending item filed only among the reasons for not doing it
  is a pending item that reads as closed.
- **The catalogue views.** Building `waveform`, `bpf`, `pianoroll` and the
  multitrack's lanes through `tree.rs` is what gives the standalone host the same
  function from the same code, and it is entangled with AP6's convergence: the
  lanes and clips are the view that has to be named first. It lands there.

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

### AP6 - `FormEditor` converges

It is ported to the seam, not ported to Rust as it stands.

- `FormEditor` becomes an application whose structure is a document; `FormEditing`
  keeps what is genuinely the tree's (the held document, the node index) and
  loses what AP0 took.
- The lanes/clips projection is a view built through AP5's path.
- The mapping rule (root aggregate -> lanes, members -> clips, a nested
  aggregate as its summary until expanded) is unchanged; expand/collapse becomes
  screen state under AP3.
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

**Acceptance:** `composer.py` and the web client's equivalent page do the same
things by the same calls in the same order, read side by side, verb by verb; the
whole loop still works - built in Python, drawn, edited by hand, heard, undone,
redone, saved, reopened. **And AP2's, which is this milestone's now:** editing
one clip in a piece of many emits no definition and no free, and a scroll
position and a selection survive a redraw of the window they are in.

### AP7 - A second application, to prove the abstraction

An application that is **not** the multitrack: a small bundle of editable
subviews over structures with nothing composed behind them - a buffer, a curve
and a timeline in one window set with one undo order. It is written as an
example (`clients/python/examples/`), in both clients, because an abstraction
with one real consumer has not been tested.

**Acceptance:** it is written with the supported surface and nothing is added to
the clients to make it possible; the two versions are one program in two
languages; an undo walks the interleaving of what was done in each subview.

### AP8 - The pass over the packages, and the plans keep what is worth keeping

The milestone that makes deleting this file legal.

- **Docs:** `docs/architecture.md` gains the application scope and its place in
  the four layers; `docs/gui-protocol.md` takes whatever the diff and the derived
  id change on the wire; the Python book's composition chapter and the web
  book's equivalent follow; `docs/bindings.md` and the parity tests take every
  new symbol. `scripts/check-docs.sh` before committing anything a book reads.
- **Decisions:** `docs/decisions.md` records the two worth recording - the
  application as the unit that owns the id space and the screen state, and a
  widget id derived from a structure's identity rather than leased.
- **The plans keep the durable half of this file:** the new track in
  `clients/gui/PLAN.md` (the seam, the derived id, the diff, the screen state),
  a pointer in `clients/python/PLAN.md` and `clients/web/PLAN.md`, and - in
  `crates/clausters-document/PLAN.md` - the answer this branch gives to the open
  question about the second document, or an honest statement of what it still
  does not settle.
- **Checks:** `cargo fmt`, `cargo clippy --all-targets` clean,
  `.claude/skills/feature-matrix/check.sh`, `npx pyright` in `clients/python`,
  `./build.sh && ./test.sh` in `clients/web` with the parity vectors regenerated,
  and the touched examples run by hand.

**Acceptance:** nothing in this file is the only copy of anything, and deleting
it loses no decision.

## Found by use

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

  Everything this turns up is recorded **here**, in this file, because all of it
  is to be dealt with — including by changing the design.

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

- ⬜ **The undo/redo walk is still each client's, and lowering it is a design
  step rather than a move** *(found 2026-09-06, scoping AP5)*.
  `Application.step` asks the editing context for a step's legs and hands them
  round the registered editors, each projecting the ones it owns through its own
  domain. Every part of that orchestrates **client objects**, so it cannot be
  lowered the way the difference was: the crate would have to drive the clients
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

## The order, and why it is this one

1. **AP0 before anything**, because moving the seam in Python is cheap and
   moving it after AP5 means moving it in Rust and in TypeScript too.
2. **AP1 before AP2**, because a diff over leased ids is a diff over noise.
3. **AP1-AP4 before AP5**, and this is the load-bearing one: lowering the core
   before the seam exists freezes the wrong unit in Rust, with the id lease
   inside it.
4. **AP6 after AP5**, because converging `FormEditor` onto a seam that is still
   moving is converging twice.
5. **AP7 after AP6**, because the second consumer is only evidence if the first
   one is already on the abstraction.
6. **AP8 last**, and the open question about the application document is
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
- [~] AP5 - the application core moves to Rust *(the difference is down; the rest is scoped below it)*
- [~] AP6 - `FormEditor` converges *(its widgets are named; the seam itself is open)*
- [ ] AP7 - a second application
- [ ] AP8 - the pass over the packages
