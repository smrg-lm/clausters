# Roadmap — the order the open work is taken in

*Rewritten 2026-08-17 against the plans and the last month of history. A rewrite
**drops what is done** and reorganizes what is left: this file is not a record
of anything, and the record of what shipped is the git history and each plan's
own checkbox.*

**This file is temporary, and it defines nothing.** It is a working sequence
over pending work that lives, already written, across several `PLAN.md` files —
milestones with their own labels, and entries in a plan's "Found by use" or
"Future directions" lists. A roadmap line says only *when* and *because of
what*; the content, the decisions and the acceptance are read in the plan that
owns it, and if the two disagree the plan wins and this file is stale, which is
the normal way for it to be wrong. When the sequence it holds is exhausted the
file goes away; nothing is ever written here first.

**The destination this order serves**, stated so an entry can be judged against
it rather than against taste: **a functionally complete example of the
arrangement, the document and the GUI together** — a composition built in
Python, drawn as a multitrack editor, edited by hand, heard, undone, redone,
saved and reopened — on a model that is **usable and correct at real sizes**,
not only at an example's. Both halves are load-bearing: today the loop runs and
the *reopening* half of it does not survive contact with a real piece.

Where the work lives:

| Track | File | What it is |
|---|---|---|
| `Ox` | `crates/clausters-document/PLAN.md` | the document: tree, intents, log, session, bindings |
| `Dx`, `Hx`, `Ax`, `Kx`, `Ex`, `Gx` | `clients/gui/PLAN.md` | the GUI host: gestures, undo from the hand, measured layers, the widget API |
| `Cx` | `clients/python/PLAN.md` | the Python client |
| `Wx` | `clients/web/PLAN.md` | the web client |
| `Mx`, `Sx`, `Tx`, `Rx` | `PLAN.md` (root) | the server |

Entries below that carry no label are **plan entries, not milestones** — they
are named by their own title and by the plan that holds them, and a phase that
takes one may well turn it into a milestone there first. **A pointer names the
plan and the section, and quotes the entry's title verbatim**, so it is found by
searching for the title rather than by reading the plan through; the sections
are "Found by use", "Future directions" and, in the document crate, "Open
decisions". If a search comes up empty, this file is stale and the plan is
right — that is the normal failure, not a sign the work vanished.

---

## Phase 1 — a saved piece comes back as the piece that was saved

*What it buys: the last step of the destination's loop. The whole-loop example
runs end to end and **reopening is where it fails** — an eye pass on 2026-08-17
over `gui_composer.py`'s four-lane piece lost the lane labels, both roll bodies,
and more of the picture on every save/open cycle. Everything here was found by
running that example, and none of it is guessed at.*

- ✅ **A document's node ids are minted per conversion and stamped on the
  object, so two compositions number from 1 and collide** — done 2026-08-17.
  *(`clients/python/PLAN.md`, found by use)*. **First of everything**, and it
  moved here the day it was measured: this phase's whole acceptance is *the same
  composition by identity*, compared tree against tree, and identity is what is
  broken. Two arrangements built in one script both hold ids 1, 2, 3, so
  material reused across them carries a number another element already holds —
  no object shared, no authoring discipline that avoids it — and nothing catches
  it: conversion reads only the maximum, the crate resolves an intent to the
  first match and the editor's index keeps the last. Every test written for the
  three entries below is written over documents whose ids are assumed unique, so
  it is cheaper than any of them and it is what makes them assertable. Its
  *decision* half — who owns uniqueness, the bridge or the crate — is written in
  the document plan's Open decisions beside the placed-twice question, and the
  cheap enforcement (unique within a document, a duplicate refused by name) does
  not wait for it.

- ✅ **A session does not round-trip: a group loses its name, and a `Track`
  comes back as a `Group`** — done 2026-08-17. *(`crates/clausters-document/PLAN.md`, found by
  use)*. The format's: a node has no name and nothing says what a set *is* to a
  view, so the fix is the schema, the crate and both clients — and the entry
  after it is read against the same format pass. Half of it is already answered
  elsewhere and should be copied rather than invented: the **server's group
  label** is a referenceable name that is not an identity, born with the group,
  renameable, clearable, and contributing a path segment that falls back to the
  decimal id when there is no name — so an anonymous node stays reachable. Its
  acceptance is a test comparing the two trees, not an eye pass.

- ✅ **What a document calls a generator is `repr()`** *(same plan, same list)* — done 2026-08-17.
  A leaf's reference is an object's `repr` when it has no name, so a saved
  session carries a memory address: the file is not deterministic between runs
  (which O1's acceptance asked for) and the reference is unresolvable by
  construction, which is why a pattern lane comes back frozen. Rides with the
  entry above — one pass over what the format carries about a leaf — and its
  acceptance is the other half of the same run: the piece plays its pattern lane
  again, and the file is byte-identical between two runs of the same script.

- ✅ **A reopened session draws less on every redefine, while the model is
  intact** *(`clients/gui/PLAN.md`, found by use)* — diagnosed 2026-08-17, and
  it was **not** the host: the trace shows every redefine fetching and placing
  its material. Both symptoms were the client's, each fixed with its own entry
  (the frozen generator, and a source table built from the material the script
  started with rather than from the tree being saved). What would reopen it is a
  pass on the fixed example that still loses drawing: the trace says the host
  redraws what it is handed, and says nothing about a tree that is handed less.

**Phase 1 is closed**, and its acceptance was run by hand rather than claimed:
three save/open cycles over the four-lane piece, every lane keeping its label,
its body and its material. What it leaves behind is one diagnosis rather than a
fix — the host was not at fault — and the next rewrite of this file drops the
phase entirely, since the plans' checkboxes and the history already carry it.

**Not in this phase, and it is the one judgement call here:** *"May one element
be placed twice, and what does an intent name if it is?"* (document plan, Open
decisions) with its defect *"Two placements of one element share a node id"*.
Both examples now give each placement its own element, so nothing is blocked;
and the faithful answer — an intent naming a **member handle** rather than a
node — is a one-tree question. It goes with the one-tree phase below, where
the other two id questions are.

## Phase 2 — a write costs the samples it writes

*What it buys: the premise the whole editing architecture was built on, corrected
before anything is refactored onto it. S12 decided how an editor writes from a
measurement — a write cost the buffer — and S14 removed the reason one day later
by making every sample an atomic cell. Nothing followed it, so today the write
path still copies the take whole and, worse, **silently discards** whatever an
in-place writer (`RecordBuf`, `BufWr`) put there since the snapshot. Short, and
it goes before the two editing phases because its decision is their input.*

- ✅ **S18 — A buffer write writes the samples it names** *(root `PLAN.md`)* — done 2026-08-17, and measured through the wire: 73.1 ms → 3.5 ms on a ten-minute take, flat in the material.
  Measured 2026-08-17, in process, so the wire is not in it: a 1 ms run on a
  ten-minute take costs **65.7 ms by copy and 0.0001 ms in place**, flat in the
  material rather than linear in it; even the whole-buffer case is **3.1x**
  faster in place, allocating nothing and touching the data once. The copy also
  drags the chaining machinery that exists only to stop copies erasing each
  other, which goes with them. Nothing on the wire changes.

- ✅ **Does the working copy still lead, now that a write costs the span?**
  *(`crates/clausters-document/PLAN.md`, Open decisions)* — taken 2026-08-17:
  **it leads, and there was never a second copy to make.** The server buffer
  *is* the working copy (loading a file into one already copied it), so a take
  is edited where it lies, with no confirmation step — the acknowledgement and
  the log entry settle a stroke, and the previous samples ride in the log
  because a destructive caller reads its span before writing. A `Temporary`
  copy stays mandatory exactly where material is reached **by reference** to
  the user's file, which is the path S19/H6 open. O8's "the working
  buffer leads while the session is open, and the pool buffer is replaced whole
  once, on confirmation" was derived from the cost that just went away. The
  answer decides whether a session still has a confirmation step and whether a
  take exists twice while it is edited — which is what the next phase would
  otherwise refactor the editor *into* without asking. Taken here, right after
  S18 makes it answerable, and not folded into either milestone.

**Phase 2 is closed.** The write path is the edit's rather than the material's,
the race that silently ate a recording is gone, and the architecture question it
existed to answer is answered — which is what the next phase would otherwise
have refactored the editor into without asking.

## Phase 3 — one tree, and undo that reaches inside a clip

*What it buys: the destination's "undone, redone" at the size a user means it.
Undo works for clips and for nothing inside one — a note edit and a break-point
edit rewrite the arrangement directly and leave no log entry — and the fix was
implemented and reverted on 2026-08-17, which is the finding that orders this
phase.*

- ⬜ **O13 — One document, held: the editor stops re-deriving what it already
  has** *(`crates/clausters-document/PLAN.md`)*. Written 2026-08-17 with the
  measurement that argues it: a gesture on a 10240-event composition costs
  **107 ms**, of which the crate's own edit is **0.014 ms** — the rest is the
  client rebuilding a document it had a moment ago, which is exactly the cost
  O12 exists to have removed. The editor holds one `Document` handle
  for the window's life and `clausters.form` becomes accessors over the crate's
  tree, rather than a parallel Python model that `_history` re-derives the
  document from on every edit. **It needs a milestone number in that plan before
  it is started**, being the largest open thing in the project and currently a
  paragraph. What already argues for it, and none of it is new: O12 removed the
  cost objection (an edit is 0.008 ms); D4's paste creates nodes the client has
  no object for; a script editing beside an open editor bumps no version, so
  O4's staleness check never fires.

- ⬜ **Undo works for clips and for nothing inside one**
  *(`clients/python/PLAN.md`, found by use)*. The vocabulary is already there —
  the roll's edit is a `SetMembers`, the curve's a `Configure` — and the revert
  says why it waits: routing them through the log took minting ids for notes,
  putting break-points into the document, and teaching `_adopt` to read both
  back out, all three of which are **reconciliation between two trees**. After
  the refactor it is the ordinary case rather than a fourth reconciliation path.
  This is what should force the refactor, and it is its acceptance by hand.

- ⬜ **May one element be placed twice, and what does an intent name if it is?**
  *(document plan, Open decisions)*, with the defect it explains. Three answers
  — forbid, copy, or **name the placement** — and the last is the faithful one
  and the expensive one, because a member has no stable identity in the document
  today. That is a property of where the tree is kept, so it is taken with the
  refactor and not before it. **Which of the three is right stopped being open on
  2026-08-17**: a clip is a window onto material and the identity is the
  material, so *forbid* and *copy* are both wrong, and the plan now carries the
  argument plus the axis it turned up — material is an **instance** (a file, a
  curve, a sequence: copies diverge) or a **function** (a pattern, a def: a
  placement is an evaluation, possibly with its own arguments), which the format
  cannot say today. What is scheduled here is the shape, not the choice. The id entry that opens this file leaves
it **wider than it was**:
  two different elements can also collide, so the decision now has to say who
  owns uniqueness within a document — and answering that in the crate is the
  cheaper half of "name the placement" rather than a separate build.

## Phase 4 — the editor answers for what it will not let you do

*What it buys: the "edited by hand" half stops teaching* sometimes it does not
work. *Small beside Phase 3 and independent of it; taken after because a
read-only body is a prop on a picture the redefine path must be drawing
correctly first.*

- ⬜ **A read-only clip body has no way to say so, so every drag on one
  flickers** *(`clients/gui/PLAN.md`, found by use — a milestone rather than a
  fix, and the plan says so)*. The refusal is correct and the acknowledgement
  carries a reason since 2026-08-17; what is missing is **earlier than the
  refusal** — nothing in the protocol tells a widget its body is read-only, so
  the roll offers the drag, draws it for its whole duration and unwinds it. The
  user's own acceptance: *if refusing is correct, the rectangles must not move*.
  The vocabulary exists (`signal` already carries `editable`) and the D2 draw
  mode landed the other half (a refusal that is visible *and* consumes the
  press). It touches the host, `docs/gui-protocol.md`, `docs/gui-props.md` and
  both clients' builders.

- ⬜ **The editing gestures want affordances** *(same list)*. Two gestures that
  work and that nothing on screen announces — the same decision as the entry
  above, seen from the other side, which is why they are one phase.

- ⬜ **E23 — the rest of the chrome answering on what it draws.** Here rather
  than in the unscheduled list now that two neighbouring entries open the same
  surface; it rides with them or it stays out, on the day.

## Phase 5 — the autonomous editor: one copy of the material, three processes

*What it buys: the GUI client as an application rather than as a window driven by
a script — the material in shared memory, NRT rendering on demand, and the RT
server as a **separate process** attached to the same segment. It is here rather
than earlier because it needs Phase 2 landed (the same write, moved across a
process boundary; doing it while the write path still copies would mean two
designs) and Phase 4's picture to be correct before the fetch under it is
deleted. It is the biggest thing on this list and the only one that is a
product goal rather than a defect.*

- ⬜ **S19 — The material lives in the shared segment, and a local peer edits it
  without a message** *(root `PLAN.md`)*. The segment already carries the control
  buses **as the very words the engine reads** — a peer's write is live on the
  next block, no command involved — and buffers are the one kind of data still
  crossing as messages. So the saving is not the copy, it is the **round trip**:
  a stroke today is a blob out, an NRT job, a whole-buffer copy, a reply and a
  reconciliation; with the samples mapped it is a store. Reads go the same way —
  a peak pyramid built over the mapped region deletes the editor's largest data
  path. What stays a message is what has semantics beyond the samples:
  allocation, lifetime, `/buffer_render`, the disk, the transport. Remote clients
  are untouched, which is why S18 comes first.

- ⬜ **H6 — The standalone editor maps what it edits, and the RT server is a
  separate process** *(`clients/gui/PLAN.md`)*. S19's other end: the host maps
  the take, draws from the mapped region, stores a stroke into the cells, and the
  engine reads those cells whether it is in this process (embed) or in another
  one on the same `--shm` path. Its acceptance is what makes "separate" a claim
  rather than a diagram: kill the RT server, start another against the same
  segment, and the editor is still drawing and editing the same material.

- ⬜ **"A finished async command waits up to 100 ms to be reported"** *(root
  `PLAN.md`, Found by use)* **comes forward from the real-sizes phase with
  this.** Once the sample traffic is gone, what a standalone app pays per
  operation is exactly that floor — an allocation, a render, a file read — and it stops hiding behind
  batched writes. Named here so pulling it forward reads as a decision.

## Phase 6 — the second half of the destination: real sizes

*What it buys: "usable and correct at real sizes". Entries rather than
milestones, in the root plan's **Found by use** list except where marked, none of
them blocking, each of them a tax paid by every session — which is why they are a
phase rather than a list.*

- ⬜ **A finished async command waits up to 100 ms to be reported.** Every
  async command pays it — a `/buffer_alloc`, a `/buffer_read`, a def compile,
  any `wait=True`. **First of the phase**: it is the cheapest and the most
  widely felt, and the fix is a wakeup when a result lands rather than a smaller
  `GC_INTERVAL` for everyone.
- ⬜ **A persisted def that no longer compiles warns on every boot, forever.**
  Seven of them on the author's machine since S17 changed `PlayBuf`'s arity.
- ⬜ **A UGen's trailing inputs could be declared optional, so a def survives
  one growing** *(root `PLAN.md`, **Future directions**)*. The cause behind the
  entry above, and the one that deletes the class rather than the noise.
- ⬜ **`transport_group` takes an id where the rest of the Python client takes a
  node** *(root `PLAN.md`, Found by use, though the fix is the Python
  client's)*. A couple of lines; the TS client's `nodeId(...)` is the shape.
- ⬜ **Ten examples play routines on a clock nobody started, and nothing says
  so** *(`clients/python/PLAN.md`, Found by use)*. Their audible half has never run — a
  mechanical fix each, plus one decision about whether playing onto a stopped
  clock should stay silent at all.

## Phase 7 — the packages move together: the arrangement reaches the web client

*What it buys: the rule the project already states, applied to the largest
outstanding violation. `form/`, `gui/editor.py`, `gui/transport.py` and
`gui/notation.py` have **no TypeScript counterpart at all** — the whole
arrangement, document and editor layer exists in one client. Last of the phases
on the path because it is a port, and porting is cheapest once the layer has
stopped moving, which is what phases 1–4 do to it.*

- ⬜ **W16 — Example parity with the Python client**, and its named track: the
  arrangement layer and the editor. The shape `gui/notation.py`'s port follows
  was decided on the Python side and is written in the web plan rather than
  re-derived.
- ⬜ **W24 — The completeness pass**, and the parity gaps that plan already
  carries with reasons (the record formatters, `defs/patch.ts`, `connectGui`,
  the two leftover names).
- ⬜ **W7** (the Faust surfaces), **W9** (MIDI), **W15** (the bundle writer) —
  unported features, each owned, none on the path to the complete example.

## Phase 8 — the spectral editor

*Everything here is genuinely later: it needs the A track's descriptors, it is
partly experimental, and none of it is on the path to the complete example.*

- ⬜ **A3 — Band-limited reconstruction and true peak.**
- ⬜ **A4 — K-weighting and the loudness family.**
- ⬜ **A5 — The loudness layer and its read-out.**
- ⬜ **A6 — The layer stack becomes explicit.**
- ⬜ **A7 — The layer stack's rules from the clients, and the books.** What is
  left of it once D8 took the client half: it rides with A6, whose rules it
  publishes.
- ⬜ **D5 — Spectral selection.**
- ⬜ **D6 — The lasso.**
- ⬜ **D7 — Spectral drawing and resynthesis** *(experimental: promoted or
  dropped on what it sounds like)*.

---

## Open, and deliberately not in the order above

Named so "not scheduled" reads as a decision rather than an oversight. Each is
in its own plan with its own reasoning; none is on the path to the complete
example, and none blocks anything that is.

- **The GUI host's own documentation** — `K16`/`K16a`/`K16b`/`K16c`. Waits on
  nothing; costs a decision about a fourth mdBook that the project's three-book
  rule does not currently allow.
- **`G31g`** — engraving refinements (tuplets, full polyphony, spelling).
- **The K track's group-aware port** — `keys`, `notes`, `patch` (item 3 of K7).
- **Server transport items** `T2`–`T4`, and `R12` (a release verifies
  something). `T2` did not stop being optional when `T5` landed, and the
  question it was flagged with still stands: `T5` put a position in samples on
  the engine, which crosses the beats↔samples conversion `T2` says is anchored
  on the wrong axis.
- **The builders could be generated from the catalog instead of contrasted
  against it** *(root `PLAN.md`, Future directions)* — the contrast tests caught
  eleven drifted builders, which is strictly weaker than not hand-writing the mirrors.
- **The level body's fade is a guess**; **a take is drawn in amplitude
  and heard in decibels**; **an element's look does not answer for the space it
  is given**; **the other text over pictures has no plate yet**; **persistence
  saves the document, not what the user did to it** — `clients/gui/PLAN.md`,
  Found by use, each with its record of what was seen; **many channels are drawn
  and not yet readable, and a take cannot be created empty** is in that plan's
  Future directions instead, being a design rather than a fix.
- **The larger "Future directions"** in each plan — the free arrangement plane,
  the in-page shared-memory path, an interpreter inside a standalone host,
  per-node staleness, more than one owner of one document, IME text, a Tauri
  wrapper, the heavy families as features, a steady goniometer.

## Revising this file

Reordering is expected and is the point: an entry moves when a dependency turns
out to run the other way, or when something found by use has to go first. What
must not happen is content migrating here — a milestone that grows a decision
grows it **in its plan**, and this file keeps naming it. **A rewrite erases what
has been done**: a closed phase leaves no line here, because the plan's checkbox
and the git history already say it shipped, and the only thing this file is for
is what is still ahead.
