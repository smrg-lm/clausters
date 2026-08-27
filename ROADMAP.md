# Roadmap — what is open, sorted by what kind of work it is

*Rewritten 2026-08-26. The previous sequence was two numbered phases and a list
of what they deliberately left out; what it stopped describing is the tree as it
stands, where almost nothing pending is a phase and most of it is either a small
gap found by use or a milestone left hanging at the edge of a closed track. So
the order is no longer by date but **by kind**: what is a fix, what is a review
somebody has to sit through, what is an unfinished milestone, and what is a
track nobody has opened. A rewrite **drops what is done** and reorganizes what
is left: this file is not a record of anything, and the record of what shipped
is the git history and each plan's own checkbox.*

**This file is temporary, and it defines nothing.** It is a working index over
pending work that lives, already written, across several `PLAN.md` files —
milestones with their own labels, and entries in a plan's "Found by use",
"Future directions" or "Open decisions" lists. A line here says only *what kind
of thing* an entry is and *what it is related to*; the content, the decisions
and the acceptance are read in the plan that owns it, and if the two disagree
the plan wins and this file is stale, which is the normal way for it to be
wrong. When what it holds is exhausted the file goes away; nothing is ever
written here first.

**The destination this order serves**, stated so an entry can be judged against
it rather than against taste: **a functionally complete example of the
arrangement, the document and the GUI together** — a composition built in
Python, drawn as a multitrack editor, edited by hand, heard, undone, redone,
saved and reopened — on a model that is **usable and correct at real sizes**,
not only at an example's.

Where the work lives:

| Track | File | What it is |
|---|---|---|
| `Ox` | `crates/clausters-document/PLAN.md` | the document: tree, intents, log, session, bindings |
| `Dx`, `Hx`, `Ax`, `Kx`, `Ex`, `Gx`, `Lx`, `Px` | `clients/gui/PLAN.md` | the GUI host: gestures, undo from the hand, measured layers, the widget API, the patcher |
| `Cx` | `clients/python/PLAN.md` | the Python client |
| `Wx` | `clients/web/PLAN.md` | the web client |
| `Mx`, `Sx`, `Tx`, `Rx`, `Bx`, `Ux` | `PLAN.md` (root) | the server, and its engine in the browser (`Bx`) |

Entries that carry no label are **plan entries, not milestones** — they are named
by their own title and by the plan that holds them. **A pointer names the plan
and the section, and quotes the entry's title verbatim**, so it is found by
searching for the title rather than by reading the plan through. If a search
comes up empty, this file is stale and the plan is right — that is the normal
failure, not a sign the work vanished.

**What gets written down, here or in a plan: what is still open, and nothing
else.** A bug found and fixed in the same pass is **not** an entry — its story
(what was wrong, why, how it was fixed) belongs in the **commit message**, which
is the record of what shipped. What may survive it is the *general* thing it
exposed: a class of problem nothing covers, a rule nobody stated, a decision
nobody took. That is written down, as an open item, and the bug is not. Closing
a checkbox that was **already** open is the other case and stays right: that
entry was pending, and it keeps the record of what was wrong.

The five sections, and the line between them:

1. **Fixes** — something is wrong, missing or duplicated, and what to do about
   it is already known. No decision stands in front of the work.
2. **Fixes that need a decision first** — the same kind of small work, except
   that the shape it takes depends on an answer nobody has given. Each one names
   *which* decision.
3. **Tests and reviews pending** — work that is not a change to the tree at all:
   somebody has to run something and watch it. It is separate because it is the
   one kind of work nothing in CI does and nothing in a plan's checkbox implies.
4. **Milestones left hanging** — numbered milestones in a plan whose track is
   otherwise closed, again split by whether a decision comes first.
5. **Tracks not started, or incomplete** — whole tracks, named and referred to
   their plan, not enumerated here.

---

## 1. Fixes

Each is small, owned by its plan, and blocked by nothing.

**Nothing open.** This section fills from section 3's review and empties again;
a fix that lands leaves no line here, because its plan's checkbox and the commit
already carry it.

## 2. Fixes that need a decision first

Same size of work, except the shape depends on an answer. The decision is named
on each one; none of them is being taken by this file.

**Nothing open.** An entry lands here when a fix turns out to need an answer
first, and leaves when the answer is given — the answer goes in the plan that
owns the work, never here.

## 3. Tests and reviews pending

Nothing here is a change to the tree. Each is somebody running something and
watching it, which is the one kind of verification this project has no automation
for — CI runs no example, and a plan's checkbox says a thing shipped, never that
a person saw it work.

- ⬜ **A manual and visual review of the whole thing, by the user, done
  together.** *This is the one that comes first in time, whatever order the rest
  of this file is read in.*

  **Why it leads.** Nothing in this project runs an example. CI does not, the
  test suites do not, and a signature change breaks them at a call site no build
  ever reaches — which is why `CLAUDE.md` calls the examples the manual test
  surface and not a decoration. Everything the last phases shipped was accepted
  by a page, a suite or a measurement, and all three of those check what somebody
  thought to check. What they cannot report is a picture that is *correct and
  wrong*: a widget that lands where nobody would put it, a take that sounds right
  and looks off by a frame, an editor whose gesture is legal and unpleasant,
  prose in a book that no longer describes what the reader sees.

  **What it is.** The user runs the examples and the pages and says what is
  wrong; I sit alongside, reproduce, and either fix on the spot or write the
  entry down. Both example directories, both clients, the host native and in a
  browser — with the pairs read against each other, since "the same example in
  two languages" is a claim this has never had a person check.

  **What comes out of it.** Not a checkbox. Each finding goes, the day it is
  found, into the "Found by use" list of the plan that owns it, with its own
  checkbox; the ones that turn out to be designs go to "Future directions". This
  entry closes when the pass is done, and section 1 is expected to grow from it.

- ⬜ **Nobody has watched the release gate stop anything** *(root `PLAN.md`,
  `R12`, the `⚠` clause)*. `R12` shipped: `verify` runs the full feature matrix
  and the tests, `build` and both `publish-*` jobs `needs:` it, and a
  `workflow_dispatch` rehearsal has been watched go green with the publish jobs
  skipped. What is unverified is the behaviour that is the gate's whole purpose —
  that a *failing* `verify` stops the run — and the obvious test is unsafe,
  because if the gate is misconfigured the run continues into PyPI and npm, which
  cannot be taken back. The plan carries the safe procedure (a fork or scratch
  repository, a deliberately broken tree, a `v*` tag) and says what does not
  count as proof. **Related:** it is filed here rather than in section 4 because
  the milestone's *code* is done; what is left is somebody watching it fail.

- ⬜ **The browser's lost release is guarded and the guard is unverified**
  *(`clients/gui/PLAN.md`, Found by use)*. A page can lose a button-up where a
  desktop window cannot, so `web::input` ends the drag itself when the DOM says
  no button is down. The reasoning is sound and nothing has exercised it: the
  case did not reproduce under Chrome DevTools. Worth either a way to drive it or
  a decision that it stays on trust, rather than sitting as code nobody has seen
  work.

## 4. Milestones left hanging

Numbered milestones whose track is otherwise closed. Each is owned and written in
its plan; the plan is where its acceptance is read.

### Ready — no decision in front of them

- ⬜ **`W7` — the Faust surfaces** *(`clients/web/PLAN.md`)*. **Down to
  `defs/boxes.ts`.** The signal API turned out to be done already
  (`defs/signals.ts` landed with W1 and has been at parity since), and the engine
  half was split off as `B5`, which is closed — a def sent as source, as a box
  tree or as a signal tree compiles and sounds in a tab, and since 2026-08-26 a
  page's offline render carries one too. The example `faust/boxes-library` rides
  with it and waits on nothing else.

- ⬜ **`W9` — MIDI: `MidiFunc` in, `MidiEvent` and MIDI destinations out**
  *(`clients/web/PLAN.md`)*. Carries two pages as part of its acceptance:
  `io/midi-responder` and `editors/pianoroll-midi`. **Related:** the Python side
  of precise MIDI timing is `C18`, which is deferred by a decision already taken
  (below) — W9 is the port of what the Python client *has*, not of what C18 would
  add.

- ⬜ **`W15` — the TypeScript bundle writer** *(`clients/web/PLAN.md`)*. **A
  node milestone**, decided 2026-08-26: a bundle is an input a static page boots
  with no interpreter, so writing one is not something a page does; the page's
  half of W15 is the in-memory mount, which writes nothing. That makes **"Node
  target"** (`clients/web/PLAN.md`, Future directions) its dependency rather
  than a someday. It also carries what is left of the `panels/standalone`
  divergence: the counterpart of `standalone.py` is a node script, and where a
  node script lives in `clients/web/examples/` — pages today — is open.

- ⬜ **`T4` — `bundle_is_governed` re-resolves the group per message** *(root
  `PLAN.md`, T track)*. A linear `find` per targeted message, bounded and
  allocation-free but `O(messages × nodes)` worst case inside a block budget.
  Hoist the group's index. The only one of the three T items that is a fix rather
  than a question.

- ⬜ **`G31g` — engraving refinements** *(`clients/gui/PLAN.md`)*. Tuplets, full
  polyphony, written duration beyond `dur`, tonal spelling, grace notes,
  articulations. Not permanent limits: a refinement pass on the same encoder,
  behind the seams that were left for it, so it extends rather than rewrites. It
  is the largest thing in this section and nothing waits on it.

### Waiting on a decision

- ⬜ **`T2` — `/transport_set`'s grid origin on the transport axis** *(root
  `PLAN.md`, T track)*. With a group bound, `originSample` is still read on the
  device axis, so the grid slides by the frozen total across a pause. No test
  pins it today.
  **The decision:** the grid semantics have to be re-derived, and `T5` moved the
  ground under them — it put a position in samples on the engine, which crosses
  the beats↔samples conversion `T2` says is anchored on the wrong axis. `T2` did
  not stop being optional when `T5` landed, and the question it was flagged with
  still stands.

- ⬜ **`T3` — classification is once, at drain** *(root `PLAN.md`, T track)*. A
  bundle scheduled before `/transport_group` binds stays on the device queue even
  if its target becomes governed. It is documented behaviour today.
  **The decision:** whether to accept it as the contract or pay for re-classifying,
  which means rewriting a queue on the audio thread — an RT-safety cost against a
  case nothing currently hits.

- ⬜ **`K16` — the host's own documentation**, and its parts `K16a`/`K16b`/`K16c`
  *(`clients/gui/PLAN.md`, K track, Part C)*. With Part A done, the extension
  recipe is a public API and has no book to live in: the wire is a page of the
  server's book, driving a host is a chapter of the Python book, the component is
  a chapter of the web book, and how the host is built is a development doc. A
  reader who wants to *write an element* has nowhere to start.
  **The decision is `K16a` and the rest follows from it:** whether the GUI host
  earns a **fourth mdBook**, which the project's three-books-one-per-platform rule
  does not currently allow — is the host a platform; what moves and what must not;
  what a fourth `book.toml`, ReadTheDocs project and generated reference cost; and
  what it is called, since it would be the first book named by role rather than by
  platform. `K16b` (the widget author's guide) and `K16c` (`examples/
  custom_element.rs`, the smallest proof a third party can do it) are clear
  whichever way it goes; only their home is not.

- ⬜ **`C44` — the inverse direction: a widget inside a def**
  *(`clients/python/PLAN.md`, the API reform track)*. Recorded as an analysis with
  a reservation rather than as work.
  **The decision:** whether to do it at all. It inverts the dependency (`defs`
  would import `gui`, where the arrangement's `gui → form` rule is the precedent
  running the other way) and it autogenerates the control's name, which is the one
  thing `/node_set` addresses by. If it is ever done, the coercion runs in one
  direction only and `name=` is mandatory.

- ⬜ **`C18` — cross-platform precise MIDI timing via in-band MIDI 2.0**
  *(`clients/python/PLAN.md`)*. **The decision is already taken, with the user,
  and it is to defer:** live OS MIDI output stays best-effort, and the
  UMP-over-our-own-transport direction has no date. It is listed so that
  "unscheduled" reads as a decision rather than an oversight.

## 5. Tracks not started, or incomplete

Whole tracks, and one design the user has asked for that is a track's worth.
**They are named and referred to their plan, not enumerated here** — a track's
milestones, their order and their acceptance are the plan's, and copying them
into this file is exactly the migration the rules forbid.

- ⬜ **The A track — what a signal measures, and the layers that show it**
  *(`clients/gui/PLAN.md`, "A track")*. `A1`/`A2` shipped (mean square in the
  pyramid, the RMS layer and the `measure` prop); everything after them is open —
  band-limited reconstruction and true peak, the loudness family, the loudness
  layer and its read-out, and the two milestones that make the layer stack
  explicit and publish its rules from the clients. **Related:** the `measure`
  docstring fix (section 1) is what ships today, not a piece of this; and the
  audio editor's layers below land on this track's stack rules.

- ⬜ **The D track's spectral half — the hand that edits data**
  *(`clients/gui/PLAN.md`, "D track")*. `D1`–`D4` and `D8` shipped (the grabbable
  sample, the draw mode, the two-axis marquee, copy/cut/paste, the editor opening
  an element as a signal view). What is left is spectral: the selection the
  `select_box` step already declines to answer, the lasso, and spectral drawing
  and resynthesis — the last of which is **experimental** in the `G20f` sense,
  promoted or dropped on what it sounds like. It needs the A track's descriptors,
  which is why the two are read together.

- ⬜ **The P track's phase B — the patcher becomes an editing surface**
  *(`clients/gui/PLAN.md`, "P track")*. Phase A is complete at both levels: a
  `GraphDef` and a `SynthDef`/`FaustDef` each have an autonomous read-only view,
  decoded headlessly, laid out by the host. Phase B is two milestones that are
  **deliberately not designed yet** — the editing and authoring surface, and where
  a patch lives when the window closes — and the plan records what the E track
  changed under them (there is no `patch` type any more; the gestures ride the
  shared machine; the eye pass is part of closing it) plus the third persistence
  option the host grew while the track was paused.

- ⬜ **The audio editor's layers: the view is the next container, and it is the
  richer one** *(`clients/gui/PLAN.md`, Future directions)*. Not a milestone —
  **a track's worth**, and the one the user asked to have thought through and
  built next. The layer mechanism is done and general (`host::layers`, proved by
  the `clip`); what this needs is the *contents*, and they are of two kinds the
  design must keep separate: **visualization layers** (the same material drawn
  several ways, alternating or superimposed) and **edit layers** (what a hand is
  doing, one at a time — non-destructive processing as curves over the waveform,
  seen, heard, then rendered in). The two combine and are not the same axis. It
  lands on `A6`, which is why the A track is above it here. Two neighbouring
  entries in the same list are part of the same design and are read with it: "The
  layer stack is one container's, and an audio editor's view has one too", and
  "Many channels are drawn and not yet readable, and a take cannot be created
  empty".

- ⬜ **The free arrangement plane (the blueprint view)** *(`clients/gui/PLAN.md`,
  Future directions)*. A **second kind of multitrack**, explicitly not a milestone
  of the one that shipped: it shares characteristics with the lane stack and its
  model differs structurally, so it is still to be defined and planned.

- ⬜ **More than one owner of the same document**
  *(`crates/clausters-document/PLAN.md`, Future directions)*. A track of its own
  if it is ever opened, and recorded so that the single-owner assumption reads as
  a deliberate floor rather than an oversight — it is what keeps operational
  transformation and CRDTs out of a design that does not have the problem they
  solve. **Related:** "Staleness per node rather than per document" in the same
  list is its seam, and buys nothing until this exists.

### The larger questions, and the plans' own Future directions

Named, not enumerated: each is written where it belongs and is read there.

- **What the *second* document is — the application's, as against the
  arrangement's** *(`crates/clausters-document/PLAN.md`, Open decisions)*. It is
  **open and undefined** by intent, it decides what the crate is for as much as
  what it stores, and it is not work to schedule. Nothing above waits on it; it is
  named so its absence reads as a decision. The `Session`/`Document` naming pass
  waits on it, and so does where a widget's left-behind value is saved.
- **The remaining "Future directions"** of each plan — the server's (a long take
  played out of the pool and `DiskIn`'s missing start frame; generating the
  builders from the catalog instead of contrasting against them), the GUI's (the
  double click and the long press, and a pass over the gesture vocabulary; a
  steady goniometer; the heavy families as features; composed text (IME); a Tauri
  wrapper; the three heavy-view rendering questions), the web client's (a node
  target, type-safe GuiDef/def schemas, a remote-server standalone page), the
  Python client's three open questions, and the document crate's interpreter
  inside a standalone host. Every one of them carries its own checkbox in its own
  plan.

---

## Revising this file

Reordering and re-sorting is expected and is the point: an entry moves between
sections when a decision is taken, when a review turns a design into a fix, or
when a track is opened. What must not happen is content migrating here — a
milestone that grows a decision grows it **in its plan**, and this file keeps
naming it. **A rewrite erases what has been done**: closed work leaves no line
here, because the plan's checkbox and the git history already say it shipped, and
the only thing this file is for is what is still ahead.
