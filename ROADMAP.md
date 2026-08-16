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

- ✅ **D8 — The editor opens an element as a signal view.** Done 2026-08-15.
  Phase 2's condition, met: `Editor.open_signal` is `open_pianoroll`'s sibling,
  and after A2 it opens an element showing both measures rather than a bare
  trace. It sat before the two editing milestones because without it a sample
  editor is a free-standing example beside the arrangement rather than a view of
  it — and it earned its keep early by correcting A2's layer stack.

- ✅ **S13 — The NRT server takes operations on demand.** Done 2026-08-15, and
  the reordering held: taken before S12 because it alone is a complete
  demonstrable thing, and it was — `/buffer_render` runs the graph into a buffer
  and the samples match the batch render of the same material, sample for
  sample. Two things it settled that the entry had wrong: the front is the
  **embedded server's**, not the `Renderer`'s (which has the synchronous
  execution and none of the transport), and *no scheduling surface* was never a
  property to want — a timetag is meaningful inside an operation, because an
  operation is a score; what the mode lacks is a clock that moves on its own.
  What it did **not** buy: a client still cannot reach a session, so the
  builders and the example wait for whatever gives one a door.

- ✅ **S12 — Editing does not go through the pool, and its verbs are three.**
  Done 2026-08-15. `clausters_core::edit` plus `/buffer_gain` and
  `/buffer_reverse` on the wire, with both clients' builders and the example;
  `replace` needed no command, being `/buffer_setRange` already. None of the
  verbs is a new algorithm — `gain` rides the same shape math `EnvGen` plays.
  The half of its acceptance that named a *drawn* edit belongs to D1 and D2,
  there being no pencil yet, and the plan says so rather than claiming it.

- ✅ **S15 — `/buffer_fill`, `/buffer_readChannel`, `/buffer_allocReadChannel`.**
  Done 2026-08-16. The set scsynth's is measured against is complete again, and
  one channel of a stereo take can be loaded on its own — which could not be
  done at all before, `/buffer_read` failing outright on a channel-count
  mismatch. The channel reads turned out to be one argument on the arms that
  already existed, not a second way of reading a file.

- ⬜ **S14 — A pool buffer can be allocated writable** *(root `PLAN.md`)*.
  **Not part of the editing chain** — in the interactive mode a buffer is already
  mutable, there being no audio thread, so this one is the RT server's alone. It
  is here because the pass that produced the other three is what found it: in
  SuperCollider you allocate an empty buffer, zero it and use it for anything,
  and here you cannot, so `RecordBuf`, `BufWr` and the `BufDelay*` family have
  nowhere to live. A missing capability rather than a decision, and the
  completeness the S track exists to reach. Takeable at any point; it blocks
  nothing in this phase and this phase blocks nothing in it.

- ✅ **D1 — A sample is a grabbable point.** Done 2026-08-16: the gesture, the
  pending overlay's first real drawing, and `peaks::update_range` in both
  pyramids, asserted identical to a rebuild. It turned up a hole the milestone
  had not named — an owner had no way to say *the material is now this* — so
  `data` became settable on an inline source; the same door for a **mapped**
  one is still missing and D2 needs it.

- ✅ **D2 — The draw mode.** Done 2026-08-16: a step in the plan table, one
  intent per stroke with the runs as blobs, the samples between two motion
  events filled in, and a refusal that is visible *and* consumes the press. It
  also landed the door D1 named — a mapped source can be told to read itself
  again — which is what lets a pending edit be let go of at all.

- ✅ **An edit invalidates the measures drawn over it, and nothing says so.**
  Taken 2026-08-16, at the end of the phase as intended and not found in an
  example. **The answer generalizes past loudness**, which is why it was worth
  taking here: *a measure's affected span is the edit's span widened by the
  measure's memory*. `peaks::update_range` is that rule with a memory of zero,
  which is why peak and RMS already follow an edit exactly and cheaply;
  momentary and short-term are the same rule with a real one, so they
  **recompute** over the span plus their window and neither grey nor go stale.
  The aggregates (integrated, LRA) are gated over all the material, so their
  memory is the take: they are **numbers, not layers**, and a stale number says
  so. Drawing a stale figure unmarked is the one outcome ruled out — the failure
  the entry was written about. Recorded in A5, which is where it will be read.

**Phase 3 is closed.** The sample editor exists end to end: an element opens as
a signal view, a sample is grabbed or a run is drawn, the intent reaches an
owner, the material changes, the overview follows it for the span that moved,
and the hand lets go when the edit is acknowledged. What it needed from the
server it got as its own three milestones rather than as a patch — an offline
mode that takes operations, the edit verbs, and the buffer commands that were
missing — and the two doors an owner needs to correct a picture (`data` for
inline material, `reload` for mapped) turned up by building the thing rather
than by planning it.

**S14 rides on**, listed above and deliberately unfinished: it never was part of
this chain (in the offline mode a buffer is already mutable, there being no
audio thread), and it is the first thing to take next unless something else
argues for itself — a pool buffer that can be allocated writable, and with it
`RecordBuf`, `BufWr` and the `BufDelay*` family, which is a capability
SuperCollider has and this server does not.

## Phase 4 — the third writer

- ✅ **H3 — The standalone editor is its own owner.** Done 2026-08-16. The host
  links the crate, holds a document, applies its own inverses. It is what makes
  O8's acceptance true as written ("a session written by the Python client opens
  in a standalone host and vice versa") — the third writer now exists, and the
  round trip was run by hand through `gui_session.py`. It was also the forcing
  case O10 named for the host taking the intent vocabulary, which it now does.

- ⬜ **H4 — The standalone editor sounds what it edits.** Split out of H3 when
  H3's acceptance ran without it: the audible half needs a server leg the
  session mode never had, so a take draws nothing and nothing plays. Next in
  this phase, and the first thing that puts S12's editing verbs and S13's
  interactive session under a hand instead of under a test.

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
