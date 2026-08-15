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

- ✅ **O12 — An edit costs the edit, not the document.** Done 2026-08-14, and
  the ordering argument held: H2 now has a door it will keep. One `Place` over
  a 10240-event composition went from **205 ms to 0.008 ms**, and the cost no
  longer depends on the composition at all — the tree stays behind a handle,
  and an edit runs in place, rolled back by its own inverse rather than
  protected by a copy of the tree.

- ✅ **H2 — Undo and redo from the hand.** Done 2026-08-14. The `Editor` drives
  the crate's log over a `Document` held in step with the arrangement, and the
  grid moved into the crate with it — the editor states where the hand put a
  clip and `Rules { quant }` decides. Reached from a button or from Ctrl+Z,
  which the host addresses to the **window**: a gesture-plan step consumes a
  press somewhere, and undo is aimed at no place at all.

- ✅ **The whole-loop example.** Done 2026-08-14 as
  `clients/python/examples/gui_daw.py` — build, draw, edit, hear, undo, redo,
  save the session, reopen it. It is the first thing that exercises O8's format,
  O5's log and the acknowledgement in one run, and being the manual test is what
  it was for: it found three defects nothing else would have (`follow=True`
  raising before the first play, a reopened session having no drawable material
  until its sources are resolved, and two placements of one object sharing a
  node id).

- ✅ **What a clip's edge means for an element that has not been rendered.**
  Settled 2026-08-14, and the answer was already decided — by O8, C31 and the
  four-layer table, which nobody had read together. **A placement is a window
  onto an element, never a rewrite of it**: an edit to a bounced note is
  refused, a resize trims what is heard, and shortening a clip over its own
  notes keeps them (checked, and reversible).

**Phase 1 is closed.** What it opened rather than answered is in the plans'
"Found by use" lists — chiefly whether the model should let one element be
placed twice at all, which the example turned from a puzzle into a decision
with three named answers.

## Phase 2 — the DAW's editing vocabulary

*What it buys: a selection is a thing you can hold, hand to an algorithm and
paste. The crate already has all of it (O6, O7, O9) and nothing calls any of it.*

- ✅ **D3 — The selection gesture grows a second axis.** Feeds the typed
  `Selection` (O6). First of the phase because D4 has nothing to copy without it.

- ✅ **D4 — Copy, cut and paste as gestures.** Against the typed clipboard (O7).
  This is the milestone that forced O12's *one tree* question — a paste creates
  nodes on the crate's side — so it lands after O12 by construction rather than
  by preference.

**Phase 2 is closed.** It carried a third entry — *A7's client half, narrowed:
the `Editor` opens an element with a stack* — under the condition "only if Phase
3 has landed A1/A2". It was a bet that the A track would arrive first, and the
answer is to take the bet rather than to cancel it: A1/A2 open Phase 3, and the
entry becomes **D8** there, after them, with a stack it can actually show.

## Phase 3 — the sample editor

*What it buys: the second editor the four-layer model was designed for. Ordered
last of the three editing phases because it is the only one with a **server**
prerequisite, and because it is the one whose cost is not the client's.*

- ✅ **A1 — Mean square in the pyramid.** Done 2026-08-14. **First of the phase**, and it was the
  one entry here that waits on nothing at all — not the document, not the
  server, not the editor. Before D1 the reason is mechanical: A1 takes the peak
  cache from CLPK v2 to v3 and bumps `CORE_ABI_VERSION`, while D1 lands
  `peaks::update_range` over that same cache. In this order `update_range` is
  written once, over all three statistics. In the other order it is written
  twice.

- ✅ **A2 — The RMS layer, and the `measure` prop.** Done 2026-08-14. Rode with
  A1 (they are G20e's two halves) and is what proves the pyramid change by eye.

- ⬜ **D8 — The editor opens an element as a signal view.** Phase 2's condition,
  met: `Editor.open_signal` is `open_pianoroll`'s sibling, and after A2 it opens
  an element *with a stack* rather than a bare trace. It sits before the two
  editing milestones because without it a sample editor is a free-standing
  example beside the arrangement rather than a view of it.

- ⬜ **A sample write costs the whole buffer, not the samples written**
  *(root `PLAN.md`, "Found by use")*. `/buffer_setRange` replaces the buffer
  whole, so a draw stroke on a five-minute take copies ~115 MB per stroke. D1
  does not strictly block on it — the working copy leads while an edit session
  is open (O8) — but *hearing* each stroke does, and hearing it is the point of
  a sample editor. It is a server milestone and it sits here because the two
  after it are unusable live without it.

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
- ⬜ **A7 — The layer stack's rules from the clients, and the books.** What is
  left of it once D8 has taken the client half: it rides with A6, whose rules it
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
