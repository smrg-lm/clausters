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

**The destination this order serves**, restated 2026-09-06 when the arrangement
stopped being a projection: **the three classic applications over one document**
— an **audio editor**, a **multitrack editor** and a **score editor**, each
built on the session in `crates/clausters-document` (source, region, lane,
track, automation), each programmable from the GUI host and driven identically
from every client, on a model **usable and correct at real sizes** rather than
at an example's.

**What that replaced**, so the change is not silently absorbed: the destination
used to be "a composition built in Python, drawn as a multitrack editor". The
model under it was `clausters.form`, a client-side tree with the multitrack
*projected* out of it, and a projection has nowhere to keep the state a
multitrack actually has — which track a thing is on, its order, its placement,
its identity. That state ended up in the widget tree, which is drawn, and
drawing frees. `FormEditor` was removed on 2026-09-06 with its examples, its
tests and the book chapter built on it; `clausters.form` is retained frozen as a
small set of data structures with no view. The design that replaces it is
`crates/clausters-document/PLAN.md`, "The turn: the arrangement stops being a
projection" (`O21`-`O24`).

**Taken first, and everything below is read against it**: `O21`-`O23` closed
2026-09-06 and 2026-09-07, and of **`O24` — the three applications** the
**multitrack editor** is done: `O25`-`O33` put its projections, its
conversation, its playback and the application itself in the shared crates, run
by the standalone host and bound by both clients, and a saved session opens and
sounds from all three. **The application-scope track is closed**
(`clients/gui/PLAN.md`, AP track): the samples editor is the second application
in `crates/clausters-apps`, and the undo order over both is the crate's editing
context, in both clients and the standalone host. The applications after the
multitrack — a buffer editor, the notes editor, the score
editor, and which composed views get one — are milestones of
`crates/clausters-apps/PLAN.md` (`X2`-`X6`), each opened on a question rather
than on a design; the notes editor is done but for its recording, and the
score editor is built and waits for its eye and ear pass.

Where the work lives:

| Track | File | What it is |
|---|---|---|
| `Ox` | `crates/clausters-document/PLAN.md` | the document: tree, intents, log, session, bindings |
| `Xx` | `crates/clausters-apps/PLAN.md` | the applications over the document, each written once |
| `Dx`, `Hx`, `Ax`, `Kx`, `Ex`, `Gx`, `Lx`, `Px`, `Nx` | `clients/gui/PLAN.md` | the GUI host: gestures, undo from the hand, measured layers, the widget API, the patcher, the score model |
| `Cx` | `clients/python/PLAN.md` | the Python client |
| `Wx` | `clients/web/PLAN.md` | the web client |
| `Mx`, `Sx`, `Tx`, `Rx`, `Bx`, `Ux` | `PLAN.md` (root) | the server, and its engine in the browser (`Bx`) |
| `APx` | `clients/gui/PLAN.md` (AP track) | the application scope: the window set, the derived widget id, the reconcile, one undo order |

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

The three sections, and the line between them:

1. **Fixes** — something is wrong, missing or duplicated, and what to do about
   it is already known. No decision stands in front of the work.
2. **Tests and reviews pending** — work that is not a change to the tree at all:
   somebody has to run something and watch it. It is separate because it is the
   one kind of work nothing in CI does and nothing in a plan's checkbox implies.
3. **Milestones left hanging** — numbered milestones in a plan whose track is
   otherwise closed, split by whether a decision comes first.

## 1. Fixes

Each is small, owned by its plan, and blocked by nothing.

- ⬜ **A fade's curve drawn and heard** *(`crates/clausters-document/PLAN.md`,
  Found by use, "A fade's curve is editable only if the curve drawn is the
  curve heard")*. A join's seams play linear whatever the box drew; the
  easiest of the two fixes goes first. **Related:** the next entry, the same
  fades.
- ⬜ **The crossfade switch acts on the edits that follow**
  *(`crates/clausters-document/PLAN.md`, Found by use)*. The rule moves from
  reading the multitrack to editing it.
- ⬜ **A history step is whole on each structure, not across them**
  *(`crates/clausters-apps/PLAN.md`, Found by use)*. A step spanning a
  multitrack and a curve can stop half way; the fix puts back what it already
  stepped.

A fix that lands leaves no line here, because its plan's checkbox and the commit
already carry it.


## 2. Tests and reviews pending

Nothing here is a change to the tree. Each is somebody running something and
watching it, which is the one kind of verification this project has no automation
for — CI runs no example, and a plan's checkbox says a thing shipped, never that
a person saw it work.

- ⬜ **Nobody has watched the release gate stop anything** *(root `PLAN.md`,
  `R12`, the `⚠` clause)*. `R12` shipped: `verify` runs the full feature matrix
  and the tests, `build` and both `publish-*` jobs `needs:` it, and a
  `workflow_dispatch` rehearsal has been watched go green with the publish jobs
  skipped. What is unverified is the behaviour that is the gate's whole purpose —
  that a *failing* `verify` stops the run — and the obvious test is unsafe,
  because if the gate is misconfigured the run continues into PyPI and npm, which
  cannot be taken back. The plan carries the safe procedure (a fork or scratch
  repository, a deliberately broken tree, a `v*` tag) and says what does not
  count as proof. **Related:** it is filed here rather than in section 3 because
  the milestone's *code* is done; what is left is somebody watching it fail.


## 3. Milestones left hanging

Numbered milestones whose track is otherwise closed. Each is owned and written in
its plan; the plan is where its acceptance is read.

### Waiting on a decision

- ⬜ **The `N` track's last two — a foreign score's layout, and a score as a
  box of the multitrack**, `N7` and `N9` *(`clients/gui/PLAN.md`, "N track —
  notation: the score model, and what is written on it")*. What a page lets a
  hand do is answered: `N8` closed 2026-10-05 as the score editor's `X5.0`,
  and `X5` built the rest of the hand on it. Neither of the two left is about
  the hand, and `N7` is a **decision** before it is work.
  **`N7`** — what opening somebody else's score should preserve, since the
  reader stores an engraver's beams and page breaks as though a writer had
  chosen them; no longer invisible, since the score editor opens a foreign
  file, lays it out in runs at its page breaks and changes its rhythms.
  **`N9`** — a score as a box of the multitrack, drawn the way a box of notes
  is drawn as a piano roll. **Related:** `N9` is built -- the reading it
  draws its notes with landed first, as "From the roll to the score"
  (`crates/clausters-apps/PLAN.md`) -- and what is left of it is the eye pass
  over `editors/edit_multitrack`, and what its entry lists as left open.

- ⬜ **The applications after the multitrack, `X2`-`X6`, and the multitrack
  continued, `X9`**
  *(`crates/clausters-apps/PLAN.md`, "The milestones")*. Each is written with
  what exists under it and what is open, and each opens on a decision:
  **`X2`** a buffer editor that draws a table by
  hand, where the wavetable conversion and what the hand edits are open, and
  which takes the generation `/gui_ack` carries and nothing reads; **`X3`**
  the notes editor, done but for its recording (`X3.10`), which waits for
  `T10` and is skipped for now; **`X5`** the score editor,
  built whole (`X5.0`-`X5.9`) and not closed: what is left of it is the eye
  and ear pass over `notation/score_editor` — the entries it wrote under
  "Found by use" are all closed, and what it left as design is under Future
  directions; **`X6`** which composed views (scope, plot, waveform,
  spectrogram) get an application, and with it whether the heavy families
  become features a build can drop; and **`X9`**, the multitrack editor
  continued, whose first part is the clone (`X9.1`: a new sequence made from a
  clip or a stretch of one), postponed by the user until the multitrack's
  development resumes and waiting on `O21`(a). **Related:** an application
  inside another, under "The larger questions" below.

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

### The near work

- ⬜ **The D track's spectral half — the hand that edits data**
  *(`clients/gui/PLAN.md`, "D track")*. `D1`–`D4` and `D8` shipped (the grabbable
  sample, the draw mode, the two-axis marquee, copy/cut/paste, the editor opening
  an element as a signal view). What is left is spectral: the selection the
  `select_box` step already declines to answer, the lasso, and spectral drawing
  and resynthesis — the last of which is **experimental** in the `G20f` sense,
  promoted or dropped on what it sounds like. It needs the A track's descriptors,
  which shipped with that track (`clients/gui/PLAN.md`, A track) and are what
  the spectral half is read against.

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
  lands on the rules `A6` made explicit — the stack's order, the layer that
  owns the vertical, and the alpha — which shipped and are what the contents
  are now designed over. Two neighbouring
  entries in the same list are part of the same design and are read with it: "The
  layer stack is one container's, and an audio editor's view has one too", and
  "Many channels are drawn and not yet readable, and a take cannot be created
  empty". **`O24` now designs the same thing from the document's side**, on
  Sonic Visualiser's pane/layer shape - layers that display audio and layers
  that annotate it as one kind on one axis, with the rule that *the axis belongs
  to the pane, not to the content*. The two entries are one design and the host's
  is the view half; read them together before either starts.

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

- ⬜ **The mapping exists and is private to the `Editor`, so every example that
  plays writes a worse one** *(`clients/python/PLAN.md`, Future directions)*. A
  design, not a fix. The public verb it asked for is answered — a `Timeline`
  plays itself and seeks — and two things stand: the roll's notes → timeline
  conversion is private to the notes editor, and `Ppar`/`Pmono` are a separate
  pattern-side question.

- ⬜ **`G43` — text is shaped**, `G43a` and `G43b` *(`clients/gui/PLAN.md`,
  "G43 — Text is shaped: every script a field accepts is drawn right")*. A
  field accepts every script since composed input closed, and the host draws
  only Latin, Greek and Cyrillic without marks right: `G43a` shapes and
  reorders labels, `G43b` makes the field's caret and selection walk the
  shaped text. **Related:** the bundled face's coverage and size, which `G43a`
  decides with measured numbers.

### The larger questions, and the plans' own Future directions

Named, not enumerated: each is written where it belongs and is read there.

- **What the *second* document is — the application's, as against the
  arrangement's** *(`crates/clausters-document/PLAN.md`, Open decisions)*. It is
  **open and undefined** by intent, it decides what the crate is for as much as
  what it stores, and it is not work to schedule. Nothing above waits on it; it is
  named so its absence reads as a decision. The `Session`/`Document` naming pass
  waits on it, and so does where a widget's left-behind value is saved.
  **Restated 2026-09-06, because the turn changed the question without answering
  it:** there is no longer one document with a second beside it. `O24` names
  three applications and `O21`(c) names their documents - the multitrack's
  session, the audio editor's, the analysis layers of the audio editor, the score
  editor's - so what is open is now *what they share*, which is the same decision
  seen from the other end.
- **An application inside another, so an application is also a composed
  widget** *(`crates/clausters-document/PLAN.md`, Future directions)*. What would
  let the notes editor (`X3`) stand inside a multitrack or a script's window, as a
  composed widget. Nothing about it is designed: a subtree rather than a window,
  ids a parent hands a child, events routed to the application that owns each
  widget, and who answers a key when applications nest -- a window's own keys
  are built (its `keys` scopes), and nesting is what is left of them.
- **The remaining "Future directions"** of each plan — the server's (a long take
  played out of the pool -- a seek, and a join over files; generating the
  builders from the catalog instead of contrasting against them), the GUI's (a
  steady goniometer; a Tauri wrapper, only a possibility; a key in a tip; the page's clipboard
  reaching the browser's; a selected staff edited by its line count; a paste
  that could make tracks), the web client's (a node
  target, type-safe GuiDef/def schemas, a remote-server standalone page), the
  Python client's three open questions, the document crate's (an
  interpreter inside a standalone host; a region's level on the top of its
  fade trapezoid), and the applications' (time-stretch from a clip's
  edge; what a bare window leaves to its
  handle; a box of notes that chooses its synth;
  effects in preview and several files in one
  audio editor; whether a roll with no sequence stays; a roll's tempo and
  barlines as parameters; the score editor's
  editing rules as modules; what a score editor's hand expects and this one
  has not; dynamics as a model of the instrument and of hearing; a glissando
  that arrives without a new attack). Every one of them carries its own checkbox in its own
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
