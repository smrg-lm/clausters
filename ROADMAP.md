# Roadmap — the order the open work is taken in

*Rewritten 2026-08-25, four times: the fourth when **W16 closed** — every
script that can have a page has one, and the eleven that cannot are each
accounted for in the plan. Before that, the third, when `panels/` finished and
W16's remaining order was written down — including the one verb a page of it was
waiting on. Before that, the second, when W24 closed — its four loose ends and
the sweep that says there are no more — and W16's porting began, which is what
leads the phase now. Before that, the same day, when the web smokes got a runner and a CI step — and,
with them, a clippy sweep the toolchain forced: rustc 1.98 turned a green tree
red on a commit that touched none of it. Before that: 2026-08-24, six times: the sixth when the examples pass closed — one
folder per subject in both clients, the `gui_` prefix gone, and the pairing
countable for the first time, which is what W16 was waiting for. Before that,
the same day, when the button question closed
whole — its interface half, three verbs over events no binding touches, which
emptied the phase down to that pass. Before that, the same day, when the
same question was answered on its server side: a mode saying which pointer
primitive reaches `/node_set`, and the two switches carrying a pair of values
instead of a range. Before that, the same day, when
every carrier took an
address and all six legs went loopback, which was the last entry ahead of the
consistency pass that led before this one. Before that, the same day, when the heavy
props that are not samples got the source object, which emptied the reform's
list of gaps, and before that when the control's curve and step closed. Before that: 2026-08-23 (four times: the last after the warp family reached the
core and both clients, which put the shared half of the control-range item in
place and turned up three things that had never been written down — a wire
vocabulary with three exceptions in it, a version that lives in five files, and
the range maps' missing signal side. Before that: three times, the third once
the GUI reform's port had landed whole — including the page decision, which turned out to need nothing
from the host that W4 had not already shipped — so what leads Phase 1 is the
gaps the reform wrote down rather than faked and the question about the widgets
it did not ask). Before that: 2026-08-22, when a sitting with the composer example turned
up eight defects across the host, both clients and the document writer. Before
that: 2026-08-21 (twice: the
second time when the arrangement, the editor, the transport and the engraver all
reached the web client, which is what Phase 1 was ordered for), and the same day
before it, when the phase of things found by use was taken whole — its last entry being a design question rather than a fix.
Before that: 2026-08-20, four times — when the
overview work closed, when the four
entries it had left behind were taken — the join that draws an edge, a page's
zoom while a take records, the shape a zoom asks in, and what a column claims at
a discontinuity — when the web client stopped drawing, and again on the pass
that read the whole `PLAN.md` set against this file looking for open work it
never named (before that: 2026-08-19, when the picture-reads-the-samples phase
closed; twice on 2026-08-18, when the clip's interaction rules and the
real-sizes phase closed; twice on 2026-08-17, against the plans and once the
autonomous editor closed). A rewrite **drops what is done** and reorganizes what
is left: this file is not a record of anything, and the record of what shipped is
the git history and each plan's own checkbox.*

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

**Both halves are behind the list now.** The loop runs, a saved piece comes back
as the piece that was saved, the editor is an application with its own
processes, a clip has interaction rules, and the taxes every session paid — an
async command's 100 ms floor, a killed editor's segments, a persisted def that
would not load and could not be named — are paid. The picture reads the samples
it maps instead of copying them, and the summary over them is a file it maps
too; a recording is drawn as it fills, at the frame, on any server and in a page
as well as in a window; and a page zoomed past its summary reads what it is
looking at in the shape that span wants — a finer summary, or the samples where
only they will do — recording or finished, so the browser draws what the native
window draws rather than stopping at a bucket.

**The violation that led this list is paid**: the arrangement, the document and
the editor now exist in both clients, held together by parity suites rather than
by care. **A new one leads it**, and it is the healthy kind — the reference
client was reformed and the port has not caught up, which is the build strategy
working rather than failing. So the order is: the one decision the reform could
not take from the native side, then the port of the reform, then the two gaps it
wrote down instead of faking, then the examples, and only then the destination
the phase was ordered for.
Behind them, one track that is genuinely later. Everything else is in the
section after them, named so that "not scheduled" reads as a decision.

**A larger question was opened while that phase was taken and is deliberately
not in this order**, because it is not work to schedule: what the *second*
document is — the application's, as against the arrangement's — in
`crates/clausters-document/PLAN.md`, Open decisions. Nothing below waits on it,
and it is named here only so its absence reads as a decision too.

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

## Phase 1 — the packages move together: the arrangement reaches the web client

*Why first: the rule the project already states, applied to the largest
outstanding violation. **The named track is in** — `src/form/`,
`gui/transport.ts`, `gui/editor.ts`, `gui/notation/` and `defs/patch.ts` all
landed on 2026-08-21, each with a parity suite that asserts the two clients
produce the same tree, the same document and the same engraving, and `DefPatch`
closed the one surface it named on its way past.*

***What the warp family left, on 2026-08-23.** SuperCollider's eight range
maps and the exponential-endpoint rule now live once in
`clausters_core::warp`, reached by both clients, and the three copies of that
curve the tree carried — the envelope's exponential shape, its bent one, and
the server's `XLine` — read it instead of restating it. The control's own curve
and step, which that half was built for, closed on 2026-08-24 and are gone from
this list. What the family left behind is paid too, all of it on 2026-08-24: it has its
signal side (`RangeMapUGen`, one kind carrying the map by name), the wire
vocabulary it turned up was cleaned, and the version that lived in ten files now
lives in one.*

***The violation the phase created on 2026-08-23 is already paid.** The Python
client's GUI surface was reformed — a builder returns a view that opens itself,
any root opens, ids belong to the instance, the samples are a source, a widget is
built from the control it drives and a window binds against it — and the port
landed the same day rather than being scheduled, which is the build strategy
working as written, page decision and all. Both gaps it wrote down instead of
faking closed on 2026-08-24 — the control's curve and step, and the source for
the heavy props that are not samples — so what is left of it is the question
about the widgets it did not ask.*

***The button question is answered whole**, on 2026-08-24, both halves, and it
was answered by splitting a layer rather than an element. Press and release are the primitives;
a click and a double click are *compositions* over them and belong to the
gesture machine, so a command button never was a second kind of element. What a
button's `mode` says is only which primitive reaches the server — `gate` (both
edges, an envelope's gate) or `press` (one message, the bang) — and the finding
under it is that **a widget cannot make a value instantaneous**: what is sent is
held, so the bang is a bang only against a `tr` control the server resets, and
both clients refuse the other pair rather than letting it be found by ear. Both
switches took `on`/`off` in place of the range that never fitted them. What is
left is the interface half, which now leads the phase.*

***The smokes that led this phase run now**, on 2026-08-25: one runner over
five cases (`scripts/smoke-web.sh`) in place of three scripts that ran nowhere,
and a CI job that calls it. The two bundle pages' `?smoke=1` modes fired for the
first time and passed — which is the good outcome, not the expected one. The
entry's own claim that CI ran `clients/web/test.sh` was false too: it ran
neither.*

***W16 is closed**, on 2026-08-25: every Python example that can have a page has
one — 55 pairs, from 19 — and the eleven that cannot are each accounted for in
`clients/web/PLAN.md`, three of them moved into the acceptance of the milestone
that owns them. The pass paid for itself several times over in defects nothing
else would have found: a server that adopted no default, a GUI handle that could
not draw, a clock that could not be locked, a blob with no live door, and the
host's own theme and metrics, which were wasm exports the protocol had no verb
for. What leads the phase now is **W24's successor list** — there is none, so
what is left of Phase 1 is the three unported features below and the decision
that was always meant to be last.*

***The consistency pass that led the phase is paid whole**, on 2026-08-24:
every carrier took an address and all six legs went loopback, so choosing a
transport stopped opening a port to the LAN; the typeface stopped being a wasm
export the protocol had no verb for (`/gui_font`, the same call in both
clients); the three wire selectors that carried an underscore against their own
table's rule were renamed, with the old spellings accepted for good so nothing
stored had to be rewritten; and the release's version number, which lived in ten
files with one pair checked, is written in one place and spread by one script.
What leads the phase now is the question about the widgets the GUI reform did
not ask.*

- ⬜ **W7** (the Faust surfaces), **W9** (MIDI), **W15** (the bundle writer) —
  unported features, each owned, none on the path to the complete example. Two
  of them now carry a page apiece as part of their acceptance, moved there when
  W16 closed: `faust/boxes-library` rides with **W7** (it cannot be written
  until a page has a Faust compiler) and `io/midi-responder` and
  `editors/pianoroll-midi` with **W9**.

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
  usual ones turns up** *(`clients/gui/PLAN.md`, Future directions)*. Named when
  the click closed on 2026-08-24: unlike the click, which was the machine's own
  hit test asked a second time, these need a **clock** — a press-time window and
  a timer — so both grow state the gesture machine does not have and a threshold
  somebody has to choose. No widget wants one, which is why it is a review of
  what a GUI's gesture vocabulary should be rather than two items to build.
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
  of the `Stft` machinery behind `clausters-ffi`/`libclausters`". They lived
  mid-plan and without checkboxes until 2026-08-20; the last of them is the one
  with a dependency (the inverse FFT waits on the server's `FFT`/`IFFT`).
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
