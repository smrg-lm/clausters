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
multitrack — the audio editor, a buffer editor, the notes editor, the score
editor, and which composed views get one — are milestones of
`crates/clausters-apps/PLAN.md` (`X1`-`X6`), each opened on a question rather
than on a design, and the score editor is not started.

One thing deferred to `O24` still has no step of its own: the manual surface
the reconcile has never had — no example in either client sends a second
`/gui_def` over an open window (`clients/gui/PLAN.md`, "Found by use", "The
reconcile has no example to see it in"). `AP5`'s catalogue views, deferred with
it, are the crate's since `a1e54513`.

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

## 1. Fixes

Each is small, owned by its plan, and blocked by nothing.

A fix that lands leaves no line here, because its plan's checkbox and the commit
already carry it.

- ⬜ **The API reference leaves out 26 public-named modules**
  (`clients/python/PLAN.md`, Found by use). The Python reference's module list
  is hand-written and fell behind the package; `segments`, `launch` and most
  of `gui.editing` are unpublished. Which of them are internal is settled
  while doing it; then the generator fails on any module in neither list.


## 2. Fixes that need a decision first

Same size of work, except the shape depends on an answer. The decision is named
on each one; none of them is being taken by this file.

- ⬜ **The page suite is one browser, and the second one found a defect it had
  been passing over** (`clients/web/PLAN.md`, Found by use). Chrome and Firefox
  disagree about what an API **refuses**, so a page that is wrong everywhere
  passes here whenever Chrome is the lenient one — two known instances, the
  wheel's units and closing an `AudioContext` twice.
  **The decision:** the rule the pages assert by (the mechanism, rather than the
  absence of an exception), and whether a by-hand Firefox pass over the same
  pages joins the release checks the way the feature matrix does. The plan
  prices both halves; neither is typing.

- ⬜ **A clone: a new sequence made from a clip, or from a segment of one**
  *(`clients/python/PLAN.md`, Future directions)*. The arrangement can only make
  a second thing out of a first by **referring** to it; the verb that copies --
  deliberately, into a structure of its own -- does not exist. It was also the
  alternative to the windows the split now cuts (a split that clones needs no
  crate change, at the price of a cut that deletes instead of hiding), and that
  half is decided and recorded in `docs/decisions.md`; what is left here is the
  verb itself, whose three shapes are named in the plan. **The decision is
  `O21`(a), asked from the model's side**: if a region is two objects rather
  than one, swapping what fills a slot while keeping the slot is a copy by
  construction, and the verb's shape follows that answer.


## 3. Tests and reviews pending

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
  count as proof. **Related:** it is filed here rather than in section 4 because
  the milestone's *code* is done; what is left is somebody watching it fail.


## 4. Milestones left hanging

Numbered milestones whose track is otherwise closed. Each is owned and written in
its plan; the plan is where its acceptance is read.

### Waiting on a decision

- ⬜ **The `N` track's second half — notation: what a page lets a hand do**,
  `N7`-`N9` *(`clients/gui/PLAN.md`, "N track — notation: the score model, and
  what is written on it")*. `N1`-`N6` closed on 2026-08-29/30: the model, its
  verbs, the emission, the interpreter, the reader and the enriched forward
  path. The three that follow were the notation lines left standing under
  `G31`, plus one the editor example turned up, and each is numbered now
  because each is a **decision** before it is work.
  **`N7`** — what opening somebody else's score should preserve, since the
  reader stores an engraver's beams and page breaks as though a writer had
  chosen them. **`N8`** — which element admits which edit, where today a page
  is editable or it is not. **`N9`** — the score as an element of the
  arrangement, the question the multitrack and piano-roll views already
  answered for their own material.

- ⬜ **The applications after the multitrack, `X1`-`X6`**
  *(`crates/clausters-apps/PLAN.md`, "The milestones")*. Each is written with
  what exists under it and what is open, and each opens on a decision:
  **`X1`** the audio editor, whose requirements are stated (cut, copy and paste
  over segments, mix, a history in memory and on disk) and whose memory/disk
  split and segment model are not, and which takes the generation `/gui_ack`
  carries and nothing reads (`clients/gui/PLAN.md`, Found by use); **`X2`** a buffer editor that draws a table by
  hand, where the wavetable conversion and what the hand edits are open; **`X3`**
  the notes editor, decided to be an application, opening on what the crate
  edits; **`X4`** whether the points editor is one; **`X5`** the score editor over
  the `N` track; **`X6`** which composed views (scope, plot, waveform,
  spectrogram) get an application. **Related:** an application inside another,
  under "The larger questions" below.

- ⬜ **`G36` - `G38` — key bindings, menus and a tooltip, each a design first**
  *(`clients/gui/PLAN.md`, sections "G36", "G37", "G38")*. **The decision is the
  design**, and it follows the conventions common to desktop and mobile
  interfaces. **`G36`** — key bindings set outside the code (a config file, a
  GuiDef) instead of spelled in each element. **`G37`** — a drop-down menu over a
  tree of options and a menu bar, separable, and the existing combobox's list
  that opens out of line and shows the chosen name twice put right. **`G38`** — a
  tooltip. `G37`'s keyboard and `G38`'s text both read `G36`'s table, so the three
  are designed together.

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

### The near work

- ⬜ **`C54` - a timeline plays what is under the cursor, and an edit reaches
  the pass that is running**, with its port **`W31`**
  *(`clients/python/PLAN.md`; `clients/web/PLAN.md`)*. Opened 2026-09-08 by the
  user, hearing the by-ear check of the host's cursor work: a clip dragged while
  the line is about to reach it goes on sounding where it no longer is. Not the
  branch's, and near work all the same - it is the **sound** half of the rule
  whose drawn half shipped that day ("One cursor, it is the transport's, and the
  content never moves it", `clients/gui/PLAN.md`), and `O24`'s multitrack is an
  editor that rewrites a playing timeline by hand, which is exactly what
  `seq.timeline` was not written for -- a mismatch and not a verdict on the
  object.

  **Its first question was answered on 2026-09-08 and most of it moved into
  `O24`.** The multitrack plays through the **server's transport** rather than
  scanning a queue in a client, so the audio half needs no player anywhere. What
  is still `C54`/`W31` is the **events** half -- a region of notes fires voices,
  so it keeps a queue on `/sched_atTransport` and a re-cue on a locate -- which
  is now near `O24` rather than before it, and small enough to land with the
  multitrack's roll lane. Its reproduction is the plan's "A pass re-cued from
  the playhead drops the clip the playhead is inside", whose rule (discrete
  contents start at the next onset) was decided with the `Timeline` on
  2026-09-17. **The ground moved under it that day**: `Playhead` is gone, and a
  `Timeline` on a server transport re-cues on a locate with `/sched_clear
  "transport"`, so the milestone's "what is there today" is to be re-read
  against that before it is taken up.

- ⬜ **Try cubic instead of straight segments where the samples are joined**
  *(`clients/gui/PLAN.md`, "Found by use")*. A drawing trial for the sample
  layer only: the sub-two-sample columns and the joined segments take the same
  curve, and `signal` stays the reconstruction. **Related:** the entry below.

- ⬜ **The reconstruction draws nothing zoomed out**
  *(`clients/gui/PLAN.md`, "Found by use")*. A cache-format change rather than
  a drawing one: the reconstructed envelope as a plane in the peak pyramid,
  computed once when the cache is built. **Related:** the A track's `A3`, which
  left the filter it needs (`clients/gui/PLAN.md`); independent of `A4`.

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
  is the view half; read them together before either starts. **Related:** `X1`,
  the audio editor as an application (`crates/clausters-apps/PLAN.md`).

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

- ⬜ **A roll that sounds shows no cursor, and what can drive the line is a
  `Playhead`** *(`clients/gui/PLAN.md`, Future directions)*. **Related:** the
  same question from the view's side. The host's half is done and general
  (`PlayheadSync` drives any widget's line, and a timeline gives it a
  position); what it cannot do is follow a pattern player, which is
  forward-only and has no position — the question this one owns.

- ⬜ **`M35` — MPE: the expression belongs to the note** *(root `PLAN.md`,
  "Future milestones (M9+)")*. A milestone in an otherwise closed track,
  opened 2026-09-19, and listed last because nothing above waits on it. Its
  decisions are taken rather than pending — the internal model, the ABI the
  shared decoder crosses on (`docs/decisions.md`, "The MPE decoder crosses as a
  handle"), the three rules that keep MIDI 1.0 working, the zone's own verb and
  the query that reads a binding back — so what is left is the work. **It moves
  four packages**, which is the reason it is not small: the server, the crate
  the decoder lands in, the Python client's output, and the GUI host, whose
  `midi` feature is on by default and whose live roll would paint an MPE note
  flat. **Related:** "MIDI is missing here entirely, not only MPE"
  (`clients/web/PLAN.md`, Parity gaps), which is deliberately *after* this one —
  the web client ports a Python leg that already carries per-note expression,
  so the port crosses once; and `C18` above, the deferred MIDI-timing decision,
  which this does not reopen.

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
  widget, and who answers a key when applications nest, which meets `G36`.
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
