# Roadmap — the order the open work is taken in

*Rewritten 2026-08-25, when the browser engine stopped being a smaller engine:
`B6` gave a page the thread that is neither audio nor interface, and with it a
budgeted serving turn, its own filesystem behind `/buffer_allocRead`, and
`diskIn`/`diskOut` in a tab; `B5` then gave it Faust in all three def forms,
read by the server's own interpreters rather than by a second reading of the
schema. A rewrite **drops what is done** and reorganizes what is left: this file
is not a record of anything, and the record of what shipped is the git history
and each plan's own checkbox.*

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
not only at an example's.

Where the work lives:

| Track | File | What it is |
|---|---|---|
| `Ox` | `crates/clausters-document/PLAN.md` | the document: tree, intents, log, session, bindings |
| `Dx`, `Hx`, `Ax`, `Kx`, `Ex`, `Gx` | `clients/gui/PLAN.md` | the GUI host: gestures, undo from the hand, measured layers, the widget API |
| `Cx` | `clients/python/PLAN.md` | the Python client |
| `Wx` | `clients/web/PLAN.md` | the web client |
| `Mx`, `Sx`, `Tx`, `Rx`, `Bx` | `PLAN.md` (root) | the server, and its engine in the browser (`Bx`) |

Entries below that carry no label are **plan entries, not milestones** — they
are named by their own title and by the plan that holds them, and a phase that
takes one may well turn it into a milestone there first. **A pointer names the
plan and the section, and quotes the entry's title verbatim**, so it is found by
searching for the title rather than by reading the plan through; the sections
are "Found by use", "Future directions" and, in the document crate, "Open
decisions". If a search comes up empty, this file is stale and the plan is
right — that is the normal failure, not a sign the work vanished.

**One larger question is deliberately not in this order**, because it is not
work to schedule: what the *second* document is — the application's, as against
the arrangement's — in `crates/clausters-document/PLAN.md`, Open decisions.
Nothing below waits on it, and it is named here only so its absence reads as a
decision too.

---

## Phase 1 — the packages move together: the arrangement reaches the web client

*The named track is in, and so is the reform's port. What is left of the phase
is one pass that only a person can do, three unported features and the decision
that was always meant to be last.*

- ⬜ **A manual and visual review of the whole thing, by the user, done
  together.** *First, and ahead of every feature below.*

  **Why it leads.** Nothing in this project runs an example. CI does not, the
  test suites do not, and a signature change breaks them at a call site no build
  ever reaches — which is why `CLAUDE.md` calls the examples the manual test
  surface and not a decoration. Everything the last phases shipped was accepted
  by a page, a suite or a measurement, and all three of those check what
  somebody thought to check. What they cannot report is a picture that is
  *correct and wrong*: a widget that lands where nobody would put it, a take
  that sounds right and looks off by a frame, an editor whose gesture is legal
  and unpleasant, prose in a book that no longer describes what the reader sees.

  **What it is.** The user runs the examples and the pages and says what is
  wrong; I sit alongside, reproduce, and either fix on the spot or write the
  entry down. Both example directories, both clients, the host native and in a
  browser — with the pairs read against each other, since "the same example in
  two languages" is a claim this has never had a person check.

  **What comes out of it.** Not a checkbox. Each finding goes, the day it is
  found, into the "Found by use" list of the plan that owns it, with its own
  checkbox; the ones that turn out to be designs go to "Future directions". This
  entry closes when the pass is done, and the work it turns up is ordered after
  it — which is the only honest reason the features below are not first.

- ⬜ **W7** (the Faust surfaces), **W9** (MIDI), **W15** (the bundle writer) —
  unported features, each owned, none on the path to the complete example. Two
  of them carry a page apiece as part of their acceptance: `faust/boxes-library`
  rides with **W7**, and `io/midi-responder` and `editors/pianoroll-midi` with
  **W9**.

  **W7 is down to `defs/boxes.ts`.** Its signal API turned out to be done
  already — `defs/signals.ts` landed with W1 and has been at parity since — and
  its engine half was split off as **`B5`** (root `PLAN.md`, B track), which is
  now closed: a def sent as source, as a box tree or as a signal tree compiles
  and sounds in a tab, read by the server's own interpreters, and since
  2026-08-26 a page's **offline** render carries one too. So
  `faust/boxes-library`, which is an NRT script, waits on `defs/boxes.ts` and on
  nothing else.

- ⬜ **C44 — the inverse direction: a widget inside a def** *(`clients/python/
  PLAN.md`, the API reform track)*, deliberately last and possibly never: it is
  recorded as an analysis with a reservation, since it inverts the dependency
  (`defs` would import `gui`) and autogenerates the one thing `/node_set`
  addresses by.

## Phase 2 — the spectral editor

*Everything here is genuinely later: it needs the A track's descriptors, it is
partly experimental, and none of it is on the path to the complete example.*

- ⬜ **A3 — Band-limited reconstruction and true peak.**
- ⬜ **A4 — K-weighting and the loudness family.**
- ⬜ **A5 — The loudness layer and its read-out.**
- ⬜ **A6 — The layer stack becomes explicit.** It now has a mechanism to be
  explicit *with*: the edit-layer rule shipped general (`host::layers`), and what
  A6 grows is the contents — a view whose automation is a body rather than a
  widget beside it (`clients/gui/PLAN.md`, Future directions, "The layer stack is
  one container's, and an audio editor's view has one too").
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
- **The double click and the long press, and whichever gestures a pass over the
  usual ones turns up** *(`clients/gui/PLAN.md`, Future directions)*. Unlike the
  click, which was the machine's own hit test asked a second time, these need a
  **clock** — a press-time window and a timer — so both grow state the gesture
  machine does not have and a threshold somebody has to choose. No widget wants
  one, which is why it is a review of what a GUI's gesture vocabulary should be
  rather than two items to build.
- **`G31g`** — engraving refinements (tuplets, full polyphony, spelling).
- **Server transport items** `T2`–`T4`, and `R12` (a release verifies
  something). `T2` did not stop being optional when `T5` landed, and the
  question it was flagged with still stands: `T5` put a position in samples on
  the engine, which crosses the beats↔samples conversion `T2` says is anchored
  on the wrong axis.
- **A long take is played out of the pool, and `DiskIn` cannot be positioned**
  *(root `PLAN.md`, Future directions)* — a five-minute stereo take is 110 MB of
  pool and a thirty-minute one 660 MB, per take, and the streaming pair that
  exists for exactly this cannot start anywhere but the beginning. The smallest
  step (a start frame on `DiskIn`) is probably a U-track item; the seek under a
  moving playhead is a design, and the mapped read stays named and unchosen.
  Nothing on the path to the complete example is long enough to force it, which
  is the only reason it is here.
- **The three open questions of the Python plan** *(`clients/python/PLAN.md`,
  Found by use)* — "Acceptable equivalence level for higher math vs Faust",
  "Whether a separate `cdylib` for `clausters-ffi` is preferable" and "The
  FFI-overhead threshold at which the scalar builtin uses a pure-language
  fallback". Each is a number or a decision that nothing currently waits on;
  they are named so that "unanswered" reads as a decision rather than an
  oversight. **`C18`** sits with them, deferred by a decision taken with the
  user: live OS MIDI output stays best-effort, and the UMP-over-our-own-transport
  direction has no date.
- **The builders could be generated from the catalog instead of contrasted
  against it** *(root `PLAN.md`, Future directions)* — the contrast tests caught
  eleven drifted builders, which is strictly weaker than not hand-writing the mirrors.
- **The heavy views' three rendering questions** *(`clients/gui/PLAN.md`, Future
  directions)* — "Cache lifecycle", "Spectrogram scaling" and "Migrating the rest
  of the `Stft` machinery behind `clausters-ffi`/`libclausters`". The last of
  them is the one with a dependency (the inverse FFT waits on the server's
  `FFT`/`IFFT`).
- **The web client's three directions** *(`clients/web/PLAN.md`, Future
  directions)* — "Node target" (true in the harness, not a product), "Type-safe
  GuiDef/def schemas" (narrower than it was: the question is which source
  generates them, not whether one exists) and "A remote-server standalone page"
  (what W4 left is a destination seam in place of the page's singletons).
- **Many channels are drawn and not yet readable, and a take cannot be created
  empty**, **time-stretch: an edge that changes the material rather than the
  window** and **the audio editor's layers: the view is the next container, and
  it is the richer one** — `clients/gui/PLAN.md`, Future directions, being
  designs rather than fixes. The last of those is the one the user asked to have
  thought through and built next, and it is a track's worth: the same layer
  mechanism with richer contents, visualization layers kept separate from edit
  layers.
- **The larger "Future directions"** in each plan — the free arrangement plane,
  an interpreter inside a standalone host, per-node staleness, more than one
  owner of one document, IME text, a Tauri wrapper, the heavy families as
  features, a steady goniometer.

## Revising this file

Reordering is expected and is the point: an entry moves when a dependency turns
out to run the other way, or when something found by use has to go first. What
must not happen is content migrating here — a milestone that grows a decision
grows it **in its plan**, and this file keeps naming it. **A rewrite erases what
has been done**: a closed phase leaves no line here, because the plan's checkbox
and the git history already say it shipped, and the only thing this file is for
is what is still ahead.
