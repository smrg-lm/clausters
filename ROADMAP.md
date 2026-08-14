# Roadmap — the order the open milestones are taken in

*Opened 2026-08-14, after closing the document crate's O1–O11. Its one job is
**ordering**: which of the milestones already written in the `PLAN.md` set is
taken next, and why that one before the others.*

**This file defines nothing.** Every entry below is a milestone that already
lives in a plan, named by its own label, and the plan is where its content,
its decisions and its acceptance are read. A roadmap entry says only *when*
and *because of what*. If the two ever disagree, the plan wins and this file
is stale — which is the normal way for it to be wrong, and the reason nothing
here is ever the only place something is written down.

**The destination this order serves**, stated so an entry can be judged
against it rather than against taste: **a functionally complete example of the
arrangement, the document and the GUI together** — a composition built in
Python, drawn as a multitrack editor, edited by hand, heard, undone, redone,
saved and reopened — on a model that is **usable and correct at real sizes**,
not only at an example's. The second half is why the efficiency work sits
where it does rather than at the end.

Where the milestones live:

| Track | File | What it is |
|---|---|---|
| `Ox` | `crates/clausters-document/PLAN.md` | the document: tree, intents, log, session, bindings |
| `Dx`, `Hx`, `Ax`, `Kx`, `Ex` | `clients/gui/PLAN.md` | the GUI host: gestures, undo from the hand, measured layers, the widget API |
| `Cx` | `clients/python/PLAN.md` | the Python client |
| `Wx` | `clients/web/PLAN.md` | the web client |
| `Mx`, `Sx`, `Tx`, `Rx` | `PLAN.md` (root) | the server |

---

## Phase 1 — the edit loop closes

*What it buys: the example's loop stops being one-way. Today a gesture reaches
the arrangement and the composition re-renders; after this phase the gesture is
invertible, the model is the crate's, and an edit costs the edit.*

- ✅ **H1 — The last relative payload becomes absolute.** Done 2026-08-14.
  Opened as "every payload gains its previous value" and rewritten on starting
  it: O5 already reads the inverse out of the document, so the host's previous
  value was redundant. What was left is what nothing else can do — converting
  `"transpose"` on an engraved page from a displacement to the staff position
  it reaches.

- ⬜ **O12 — An edit costs the edit, not the document.** Before H2 rather than
  after it, and this is the ordering call worth arguing. H2 is the first heavy
  consumer of the crate across the ABI — `clausters_log_apply` takes the whole
  document on **every** call, so undo and redo pay the same measured 205 ms per
  step on a 10240-event composition that a drag does. Landing H2 on the
  by-value door means writing the `Editor`'s edit path against a surface O12
  then changes, and writing it twice. Landing O12 first means H2 is written
  once, against the door it will keep.

- ⬜ **H2 — Undo and redo from the hand.** The `Editor` drives the crate's log
  (`clausters._native.Log`), and the requirement it adds to every later
  milestone: an editable widget is not closed until its id→object route is
  covered.

- ⬜ **The whole-loop example** *(closes H2 under the project's definition of
  done; likely `clients/python/examples/gui_daw.py` rather than growing
  `gui_composer.py`, which is 309 lines and already carries its own subject)*.
  Build, draw, edit, hear, **undo, redo**, save the session, reopen it, render
  again. It is the first thing that exercises O8's session format, O5's log and
  the acknowledgement in one run, and it is the manual test for all three —
  nothing in CI runs an example.

- ⬜ **What a clip's edge means for an element that has not been rendered**
  *(`clients/python/PLAN.md`, "Found by use")*. A **decision, not code**, and it
  is in Phase 1 because the example above will hit it: the arrangement tree is
  partially evaluated by design, so an edit-back onto a *bounced* generator has
  no settled meaning — the next render puts the note back. Three questions are
  written there; leaving them open makes the finished example demonstrate a
  behavior nobody chose.

## Phase 2 — the DAW's editing vocabulary

*What it buys: a selection is a thing you can hold, hand to an algorithm and
paste. The crate already has all of it (O6, O7, O9) and nothing calls any of it.*

- ⬜ **D3 — The selection gesture grows a second axis.** Feeds the typed
  `Selection` (O6). First of the phase because D4 has nothing to copy without it.

- ⬜ **D4 — Copy, cut and paste as gestures.** Against the typed clipboard (O7).
  This is the milestone that forced O12's *one tree* question — a paste creates
  nodes on the crate's side — so it lands after O12 by construction rather than
  by preference.

- ⬜ **A7's client half, narrowed: the `Editor` opens an element with a stack.**
  Only if Phase 3 has landed A1/A2; otherwise it moves with them. Named here so
  the dependency is visible rather than discovered.

## Phase 3 — the sample editor

*What it buys: the second editor the four-layer model was designed for. Ordered
last of the three editing phases because it is the only one with a **server**
prerequisite, and because it is the one whose cost is not the client's.*

- ⬜ **A sample write costs the whole buffer, not the samples written**
  *(root `PLAN.md`, "Found by use")*. `/buffer_setRange` replaces the buffer
  whole, so a draw stroke on a five-minute take copies ~115 MB per stroke. D1
  does not strictly block on it — the working copy leads while an edit session
  is open (O8) — but *hearing* each stroke does, and hearing it is the point of
  a sample editor. It is a server milestone and it sits first in this phase
  because the two after it are unusable live without it.

- ⬜ **A1 — Mean square in the pyramid.** Before D1, and the reason is
  mechanical: A1 takes the peak cache from CLPK v2 to v3 and bumps
  `CORE_ABI_VERSION`, while D1 lands `peaks::update_range` over that same
  cache. In this order `update_range` is written once, over all three
  statistics. In the other order it is written twice.

- ⬜ **A2 — The RMS layer, and the `measure` prop.** Rides with A1 (they are
  G20e's two halves) and is what proves the pyramid change by eye.

- ⬜ **D1 — A sample is a grabbable point.** The pending overlay's first real
  drawing, and `peaks::update_range`.

- ⬜ **D2 — The draw mode.** A step in the gesture plan table, one intent per
  stroke.

- ⬜ **An edit invalidates the measures drawn over it, and nothing says so**
  *(`clients/gui/PLAN.md`, "Found by use")*. Opens the moment A2 and D1 are both
  in — a measured layer over material the hand just changed. Recorded here so it
  is taken deliberately at the end of this phase rather than found in the example.

## Phase 4 — the third writer

- ⬜ **H3 — The standalone editor is its own owner.** The host links the crate,
  holds a document, applies its own inverses. It is what makes O8's acceptance
  true as written ("a session written by the Python client opens in a standalone
  host and vice versa") — today only two of the three writers exist, which O8's
  own entry says plainly. Also the forcing case O10 named for the host taking
  the intent vocabulary, which it still does not.

## Phase 5 — the spectral editor

*Everything here is genuinely later: it needs the A track's descriptors, it is
partly experimental, and none of it is on the path to the complete example.*

- ⬜ **A3 — Band-limited reconstruction and true peak.**
- ⬜ **A4 — K-weighting and the loudness family.**
- ⬜ **A5 — The loudness layer and its read-out.**
- ⬜ **A6 — The layer stack becomes explicit.**
- ⬜ **A7 — The stack from the clients, and the books.**
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
- **`E23`** — the rest of the chrome answering on what it draws.
- **`G31g`** — engraving refinements (tuplets, full polyphony, spelling).
- **The K track's group-aware port** — `keys`, `notes`, `patch` (item 3 of K7).
- **Server transport items** `T2`–`T4`, and `R12` (a release verifies something).
- **The web client's carried parity gaps** — the record formatters, and whatever
  each phase above leaves it, since the packages move together and a port that
  is not in the same commit is a port that drifts.
- **The larger "Future directions"** in each plan — the free arrangement plane,
  the in-page shared-memory path, an interpreter inside a standalone host,
  per-node staleness, more than one owner of one document, a Tauri wrapper.

## Revising this file

Reordering is expected and is the point: an entry moves when a dependency turns
out to run the other way, or when something found by use has to go first. What
must not happen is content migrating here — a milestone that grows a decision
grows it **in its plan**, and this file keeps naming it. When a phase closes,
its entries are ticked here and the record of what shipped stays where it
always is: the git history and the plan's own checkbox.
