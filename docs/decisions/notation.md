# Notation

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Music notation: the client engraves a display list; verovio's DeviceContext is a proven-viable path kept in reserve

Notation is engraved by [verovio](https://verovio.org), which lays out a digital
score (MEI, MusicXML, ABC or Plaine & Easie) into resolution-independent
geometry: SMuFL glyph outlines placed by transform, plus engraving strokes and
fills (staff lines, stems, beams, slurs). Two ways exist to get that geometry
onto the GPU, because verovio renders through an **abstract `DeviceContext`** —
SVG is only one backend (`SvgDeviceContext`), and `Toolkit::RenderToDeviceContext`
is public. Either parse verovio's rendered **SVG** into a display list, or
subclass the `DeviceContext` in-process and receive the draw stream directly.

**Context — no GPU API consumes notation directly.** Neither WebGPU nor WebGL2
draws paths or SVG; they draw triangles and textures. So regardless of source,
notation must become a **display list of primitives** the host tessellates: a
glyph-outline table keyed by SMuFL codepoint plus placed glyph/line/fill
primitives in page units, each carrying the MEI `xml:id` it was engraved from.
That display list is the stable seam, and the host `score` widget is its *only*
renderer (glyph outlines and fills through a fill tessellator, staff/stems as the
painter's thick-line quads — one upload, one draw, WebGL2-safe). The open question
was only which **producer** feeds it.

**Decision — ship the SVG-parse producer, keep the native `DeviceContext`
documented as viable and deferred.** The engraver is driven client-side, its
rendered SVG walked into the display list and sent; verovio stays an **optional
client dependency the host never links**, so any later client (JS, wasm) reuses
the same host renderer by sending the same display list. It began as Python-only
and has since moved down into the workspace (`clausters-notation` for the
binding, `clausters_core::notation` for the pure walk/encoder/cursor fold,
`clausters-ffi` for the C ABI), which changes where the producer *lives*, not
what it is: still SVG in, display list out, still optional and still absent from
the host. The alternative — a native `GpuDeviceContext : DeviceContext`
compiled into the workspace — is real and was **empirically proven viable**, not
assumed: verovio builds from source as a standalone library (C++20, no external
dependencies — pugixml, the tuning library and the rest are embedded; no LLVM, no
Qt), a `DeviceContext` subclass overriding its ~35 primitives compiles against the
public headers, and rendering the sample score through `RenderToDeviceContext`
yields geometry **identical** to the SVG walk (22 glyphs, 29 lines, 1 beam; the
only difference is the page-margin transform the probe did not accumulate). The
licence is compatible either way — the workspace is GPL-3.0-or-later and verovio
is LGPL-3, so linking it in is clean, no separate process required.

**Consequence.** The two producers are interchangeable behind the display list, so
the native path is a drop-in replacement for the *producer alone*, never a rewrite
of the renderer. It gives three things the SVG walk cannot: no parsing round-trip,
in-process incremental relayout, and direct **edit-back** (mutate a note, re-engrave
in memory, no re-serialize). None of those pays off until the score view becomes
interactive — and editing shipped on the SVG walk after all (G31d), which reloads
and re-engraves rather than mutating in place. What actually pins the producer is
**cross-client parity**: verovio's wasm build only emits SVG, so a native
`DeviceContext` would leave a wasm client on a *different* producer that would have
to agree with it bit-for-bit, whereas the SVG walk is one producer every client
shares (native libverovio→SVG and verovio-wasm→SVG both feed the same walk). So
the notation layer's move into `clausters-notation`/`core`/`ffi` kept the SVG
walk, and the native `DeviceContext` is a **deferred, optional producer-swap** —
decoupled from editing, not scheduled with it. The verovio clone under
`third_party/` is kept as the reference for that later build and for the
libverovio binding; nothing in the shipping path depends on it.

## Score editing: verovio's editor is dead in the released wheel (upstream `#define` bug), so the producer decides the route

verovio carries a complete editing surface of its own: `EditorToolkitShared`
implements `drag`, `set`, `insert`, `insertControl`, `delete`, `keyDown`,
`navigate`, `chain`, `commit` and **undo/redo**, reached through
`Toolkit::Edit(json)` and surfaced by the Python binding as `tk.edit(dict)` /
`tk.editInfo()`. On paper that is the whole first editing pass for the `score`
widget, client-side, with no C++ build.

**It does not work in the released wheel.** In 6.2.1 the editor instance is
created by `Toolkit::SetViewAndEditor()` inside `#if defined NO_HUMDRUM_SUPPORT`
— the wrong macro (the editor has nothing to do with Humdrum; the intent was
`#ifndef NO_EDIT_SUPPORT`). The PyPI wheel is built *with* Humdrum support, so
the guard never opens, `m_editorToolkit` stays null, and **every** `edit()`
returns `false` with an empty `editInfo()` — including parameterless actions like
`undo`, which the parser would otherwise accept unconditionally. That last detail
is what makes the diagnosis certain rather than a guess about action payloads.
The symbols are all present in the shipped `.so`, so it is not a
`NO_EDIT_SUPPORT` build; it is the guard. Upstream fixed it on 2026-05-27
(`8100cb396`, "Invert #define fix"), after 6.2.1 — the clone under `third_party/`
carries the corrected `#ifndef NO_EDIT_SUPPORT`.

**Consequence — three routes to editing, and they differ in cost, not in
capability.** (1) Ship verovio ≥ 6.3 once released: editing arrives in the Python
client for free, no build. (2) Build verovio ourselves — the deferred native
producer — which unblocks the editor *now* and additionally brings the in-process
relayout and edit-back the previous entry describes. (3) Mutate the MEI in
Python and re-engrave: `getMEI()` out, edit the XML (we own the `xml:id`s),
`loadData` back. It needs no editor toolkit and no C++ at all, at the price of a
full relayout per edit and of implementing the semantics ourselves.

Route 3's price is smaller than it sounds and should be measured before it is
assumed away: a full engrave of a six-bar page — load, lay out, render, walk the
SVG into the display list — is **~17 ms**, from MEI or from Plaine & Easie alike.
That is inside a frame budget for a page of this size, so "incremental relayout"
is worth nothing yet; it starts to matter at a page count a rehearsal score reaches,
not at the sizes the editing work will first be shaped against.

**Resolved: route 2 — vendor and build verovio ourselves.** It is the only route
that does not either wait on an upstream release or reimplement the editing
semantics, and the build turned out cheap: verovio vendors all its dependencies,
has no submodules, and needs nothing but cmake and a C++20 compiler. The
arrangement follows the one already proven for libfaust — the source is *not*
committed (a clone is ~140 MB, git-ignored under `third_party/verovio`) and
reproducibility comes from two committed files, `third_party/verovio.pin` (the
remote and the exact commit) and `third_party/build-verovio.sh` (one recipe).
`third_party/BUILD-VEROVIO.md` documents it.

The pin is a `develop` commit past the `#define` fix rather than a tag, because
no released tag contains it; **repin to 6.3.0 when it ships**, at which point the
published wheel is usable again and building stops being mandatory for the Python
client. It stays required for the native producer.

**One artifact, not one per language.** The build produces the shared
`libverovio.so` with its headers and SMuFL resources, and every consumer goes
through that: the Python client binds the C wrapper (`tools/c_wrapper.h`) with
`ctypes`, `build_native.py` stages the library and its data into
`clausters/_libs/` beside libfaust, and a wasm build would expose the same
functions. This is the libfaust arrangement reused verbatim, and it was reached
the wrong way round first — via verovio's SWIG Python module, which is what the
project's own packaging offers. That module is a trap in this context: a second
compile of the same sources, a second copy of the engine and its 12 MB of glyph
data in `site-packages`, and a distribution literally *named* `verovio` that pip
can replace with the published one, whose editor is dead. It did replace it, in
this checkout, and the editing tests began failing for an upstream reason. A
library we build, bundle and load by path has none of that ambiguity, and it
keeps the client's `dependencies = []` intact.

**Building it ourselves also means choosing what is in it,** and the importers
are independent cmake options. The build keeps MEI (the canonical format the
edit cycle round-trips through), MusicXML and `.mxl`, and the two compact
hand-typed formats, ABC and Plaine & Easie; it drops Humdrum, GABC and DARMS.
Only Humdrum is a size argument, and a real one — it vendors humlib, ~148k
lines, and dropping it takes `libverovio.so` from 21 MB to 13 MB. The other
two are noise (ABC and PAE measure ~10 KB apart) and are out because nothing
reads them. The tempting inference — that the fonts are the weight — is wrong in
the direction that matters: SMuFL *is* 12 of the 30 MB **installed**, across
2656 one-glyph XML files in five music fonts, but those compress to ~600 KB in a
wheel, so they cost disk, not download.

A last twist on the `#define` bug: in 6.2.1 the editor and Humdrum were
entangled, so this very trim would have revived the editor there. Past the pin
they are independent — which is why the build can be both small and editable
without trading one for the other.

**A second upstream bug, found once the editor was alive:** `undo` and `redo` on
an *empty* undo stack dereference it and **SIGSEGV** (reproducible, exit 139).
The irony is operational, not academic — parameterless `undo` is precisely the
probe that proves the released wheel's editor is null, and that same probe
crashes the build where it works. So our editing layer must **gate `undo`/`redo`
on `editInfo()`'s `canUndo`/`canRedo`** and never issue them blind. Note those
flags are looser than they appear: a successful `drag` leaves `canUndo` false,
yet a following `undo` succeeds and sets `canRedo`. They are a crash guard, not a
model of the stack — which means the client, not verovio, will have to track
whether there is anything to undo.

## Engraving sequencing data: target MEI, and a starting-point rhythm policy

`from_notes`/`from_timeline` turn the client's own `Event`/`Timeline` into a
score — the inverse of the score→sound flow. Two choices shape it, and the
second is a **starting point, not a fixed rule**.

**Target MEI, not ABC, even though the examples are typed in ABC.** For a human
typing, ABC's compactness wins; for a *generator*, its context-dependence is
exactly the hazard — an accidental persists through the bar, beaming is driven
by whitespace, octaves are marks. MEI spells every note's pitch (`pname`/`oct`/
`accid`) and value (`dur`/`dots`) explicitly, with no state to track, so the
encoder is a straight map from data. It is also the format the edit cycle
already round-trips through, and it needs **no `xml:id`s** — verovio mints them
on load exactly as on the ABC path, so id stability across editing is unchanged.
The generator is therefore a thin MEI writer feeding the existing `engrave`/
`Score`, not a new path.

**Written duration is `dur`, and off-grid durations tie rather than tuplet.**
The initial version engraves each note as its written `dur` (the delta), not its
sounding `sustain` — notation shows the written value, and legato/stretch are
performance nuances a score does not draw. Durations that are not a single note
value decompose into **tied** notes (a dotted value when exact), and a note
overrunning a barline splits and ties across it; anything finer than the 32nd
grid snaps to it. Both are deliberately the *simple correct* policy, not a
ceiling: reading `sustain`/`legato` for staccato and short-of-slot notes, and
detecting tuplets instead of snapping, are the undated engraving-refinements
milestone (`crates/clausters-gui/PLAN.md`, G31g). The implementation keeps the two seams
that milestone needs — the `dur`→value step (`_pieces`) and the pitch spelling
(`_spell`) — each isolated in one helper, so the refinement extends them rather
than rewriting the encoder.

## The notation C ABI has no one-shot engrave: size-then-fill needs a deterministic payload

Every entry point in `clausters-ffi` that hands back text or JSON is
**size-then-fill**: the call returns the byte count the result needs and writes
it only if it fits, so a binding sizes with a null buffer and fills with a
second call. That contract has an unstated premise — *the payload is the same on
both calls* — which every previous user of it satisfied by being a pure
function of its input.

The engraver is not. It mints a fresh `xml:id` per element on every load, and
those ids vary in **length**, so a one-shot `engrave(data) -> page JSON` would
lay out two different documents across the size call and the fill call. The
second could be a byte longer than the buffer measured for the first, at which
point the fill silently does not happen and the caller is handed a size again —
a contract that can only be used in a retry loop, for a function whose result is
supposed to be a single page.

So the ABI exposes no one-shot. A binding's one-shot is
`clausters_score_open` → `clausters_score_display_list` → `clausters_score_free`:
the ids are minted once when the handle opens, and the page is stable for as
long as it lives, so the size and the fill see the same bytes. It costs one
extra call and one extra free, and it makes every size-then-fill entry point in
the crate deterministic without exception. The Rust-side one-shot
(`clausters_notation::engrave_svg`) stays — it hands back an owned `String`, so
determinism never enters into it.

The general rule this leaves behind: **anything nondeterministic gets a handle,
not a size-then-fill pair.**

## The score model is the core's, and the engraver is a port shaped like verovio's toolkit

Engraving reaches a page and a window through completely different plumbing —
libverovio linked into a process, or the same library compiled to wasm and
reached through JS glue — and the temptation with a wire like that is to let
each side own "its" score. It is a trap, and a familiar one: what looks like
plumbing is mostly *logic*. An edit is not one call, it is an action, then a
`commit` that re-runs the layout, then a reload of the edited MEI (the
MIDI/timemap cache survives an edit, so without it a transposed note keeps
sounding at its old pitch); a document must be drawn before an edit reaches it,
or the editor reaches through drawing state the load never built and the process
dies; and undo is a stack of MEI snapshots of our own, because reloading resets
verovio's stack, its `canUndo` lies, and its `undo` on an empty stack takes the
process down. Written twice, those are two chances to get each of them wrong,
and no compiler is watching.

**Decision.** `clausters_core::notation::Score` owns all of it, generic over an
`Engraver` port; `clausters-notation` implements that port over libverovio for a
native caller, and a page implements it over the wasm build's exports. Building
the engraver — a resource path, an options JSON, whatever the binding needs —
stays with the binding, because that is the part that genuinely differs.

**The port is verovio's toolkit surface, deliberately, and not an abstract
notion of engraving.** Both implementations drive the *same* C wrapper: natively
through `tools/c_wrapper.h`, in a page through the Emscripten build's exports,
which are that same wrapper (`_vrvToolkit_edit`, `_getMEI`, `_renderToSVG`, …).
An abstraction over it would be a third vocabulary nobody speaks, and it would
have to be invented from one implementation anyway. What the port buys is not
independence from verovio — it is that the *order of the calls* has one
definition.

**Consequence.** The core's own tests drive the state machine over a fake
engraver, which is the first time any of those rules has been testable without a
C++ library present; the binding's tests keep the cases that need a real
engraver to be right about. And the two clients must engrave with **one verovio
version**: `third_party/verovio.pin` tracks the release the npm package
publishes, so a display list from a window and one from a page are comparable to
each other rather than to two different engines.

## The browser's verovio is our build, not the published one — and the SDK that makes it so

The engraver reaches a page as a wasm module, and there are two ways to have
one: take the artifact upstream publishes on npm, or compile the pinned sources
ourselves with the **Emscripten SDK**. The published one is tempting — no
toolchain, no build leg, no CI question — and it is what was chosen first,
before the two builds were actually compared.

**They are not the same build.** `third_party/build-verovio.sh` trims verovio on
purpose: `NO_HUMDRUM_SUPPORT` (which vendors humlib, 148k lines, and is worth
3 MB in the wheel), `NO_GABC_SUPPORT`, `NO_DARMS_SUPPORT` — so the library
carries MEI, MusicXML and its compressed form, Plaine & Easie and ABC, which is
the list of formats the client actually offers. Upstream's npm build makes its
own choices. Taking it would have meant one *version* at both ends and two
different builds, and what a build decides here is **which input formats
exist** — so a page would read a GABC file a window refuses. That is a
capability in one client and not the other, which this project calls a defect
rather than packaging (`CLAUDE.md`, "Non-divergence").

**Decision.** `third_party/build-verovio-wasm.sh` sits beside the native script,
reads the same `verovio.pin` and passes **the same three options**; the web
client's `build.sh` stages the pair it produces, off the slim `dist/runtime.js`
so a page that never engraves downloads nothing. The Emscripten SDK is installed
user-space under `~/.local`, the same pattern as node and libfaust; nothing in
`src/` or the test loop touches it, which is what keeps the "typescript is the
only dependency" rule intact. The fonts are deliberately *not* trimmed, by the
same argument that trims the importers: the native prefix installs the whole
SMuFL set, so cutting it in the browser would make a font a window engraves in
one a page cannot.

**Size turned out not to be an argument at all, and the estimate that said
otherwise was wrong.** Dropping `-s SINGLE_FILE=1` — upstream's release flag,
which base64s the engraver into the glue — was expected to halve the bytes.
Measured, it does not: the published module is 7.0 MB raw and 2.2 MB gzipped,
this build is 6.6 MB and 2.2 MB. Base64 costs a third on disk and gzip gives
almost all of it back, so the two are the same download. What a separate
`.wasm` still gives is a file compiled from bytes rather than decoded from text
first, and cached and served as what it is — worth having, and not worth a
decision on its own. The decision rests on the build identity above, alone.

**This is reversible, and the conditions are worth naming.** The producer of the
artifact is not visible to any TypeScript: the shell drives the six toolkit
calls through the `Engraver` port whatever compiled them, so going back to the
published packages — npm `verovio`, and `@grame/faustwasm` for the Faust
compiler — costs a change to `build.sh` and an asset path, and nothing else. Do
it if the vendored build stops being convenient **for both** artifacts: if
keeping the SDK current turns into a maintenance tax, if CI's build leg costs
more than the divergence it prevents, or if upstream's builds converge on the
options we pass. The reverse trade is the one recorded above, and it is not
free: a published artifact brings its own build's surface with it.

**Faust is the other half of this and is not settled by it.** `libfaust-wasm` is
published too (`@grame/faustwasm`), so nothing forces a vendored build there
either — but the Faust wasm has to be integrated with the Rust engine rather
than merely loaded beside it, and that integration is what will decide the
shape. It is also blocked on something a compiler artifact does not solve: the
in-page engine is the `synth,embed` build with no LLVM JIT, so a def compiled in
the page has no way to be instantiated until the engine grows one (a wasm DSP
module behind the worklet, or the Faust interpreter backend executed in our own
engine). The SDK installed here is what W7 would have had to install; the
decision it still owes is its own.

## The score model is data, and so are the operations on it — in the core, not in a client

Notation grew a **model**: a `Sheet` in `clausters_core::notation` with two
durational structures that do not contain each other, and an algebra of
operations over them. The shape of that surface was decided before any of it
was written, because everything after it lands on the decision.

**It is in `clausters-core`, not in `clausters-notation` and not in a client.**
The axis is *pure versus platform*, not *general versus notation-specific*.
`clausters-notation` is the native libverovio binding — a `build.rs`, a C++
dependency, a default-off feature — and it does not compile to wasm, while
verovio's own wasm build only emits SVG, so in a page that crate does not exist
at all. The precedent was already built: the `Engraver` **trait** is in the
core and only the layout engine is per-platform (`Toolkit` natively,
`JsEngraver` in a page). MEI names no language's types, so nothing about the
model is client-native; a Python or TypeScript module holding it would reopen
the move that put this layer in the workspace in the first place.

**The model crosses to a client by value, as data.** A client holds a plain
dict or object, not a handle. The alternative — an opaque handle with methods —
reads well for editing and badly for an algebra, because composing operations
creates intermediate scores somebody then has to own and free. By value there
are no intermediates: an operation is a function from a score to a score, a
refused one changes nothing because it was never handed over, and the question
of lifetime does not arise. The engraver stays a handle, because verovio has
state; the model has none.

**An operation crosses as data too, through one symbol.** The verb and its
parameters ride inside a payload (`{"op": "transpose", "semitones": 2}`), which
is the shape `clausters_score_edit` and the document crate's intents already
use. A new verb then costs no `docs/bindings.md` row and no `CORE_ABI_VERSION`
round — with dozens of operations ahead of it, a symbol per verb would have
been dozens of ABI rounds.

**What that costs is parity, and the cost is paid explicitly.** A data-carried
verb is invisible to `tests/bindings.rs`, which sees one symbol and no
operations, so two clients could offer different verbs with nothing going red.
This is structurally the same blindness the GUI props manifest has — it folds
every builder of one wire type into a union and so cannot see a divergence
between two of them, which is how five stood unnoticed — and the same class the
UGen contrast tests caught eleven of. So the shape came with an obligation
attached: `clausters_core_sheet_ops` publishes the catalog, and each client is
read against it by a test of its own.

**The line inside a client is: what computes is Rust, what names is the shell.**
Sugar is idiom and belongs to each language — chaining, an operator for
concatenation, a slice syntax — but only while it *assembles* an operation.
Resolving "measures 3 to 10" into a stretch of time is arithmetic against the
grid; it changes the moment a meter changes or a bar is irregular, and two
clients doing it separately disagree about which notes an edit touches, which
is a divergence nobody sees until they compare two screens.

**The strongest reason for all of it is the standalone.** A host opened on a
saved session has no client language in the process, and a score it opens has to
be editable there. That makes the completeness of the vocabulary on the Rust
side a requirement rather than a preference: the standalone holds the model and
applies the same operations through the same one door, with no client involved.

## Two durational structures, and durations as exact rationals

The model's own shape, which the milestone that built it settled and the ones
after it extend.

**The metric layout and the content do not contain each other.** Measures do not
sound: they are the layout the content is written over. Keeping them as a
structure of their own — addressable, editable, carrying meter changes and bars
whose length is not their meter's — is what makes ordinary editing expressible
at all. *Transpose measures 3 to 10* needs the measure to be an **addressing
system**, which it cannot be while it is a by-product of emission; *augment a
phrase and re-bar it* needs the content to change length while the grid stands;
*change the meter* is then an operation rather than an argument to the encoder.
It also makes **metric position** computable, which is meaning and not layout —
whether a note falls on a downbeat is a fact about the music.

The content is **flat** for the mirror reason: nesting notes inside measures
breaks under every operation that changes a length. MEI nests, so emission
*projects* the flat content onto the grid, splitting and tying at each barline —
which the encoder already did.

**Durations are exact rationals, and ticks are a boundary.** A quarter is `1/4`
and a triplet eighth is `1/12`: exact as a fraction, unrepresentable in binary
floating point and impossible on any fixed grid of ticks. The encoder used to
declare `TPW = 32` as its foundation — every duration an integer count of 32nds,
*so that* barline splitting was exact integer arithmetic — and that foundation
is what made tuplets a question about moving the base. Exactness comes from the
rational instead, and ticks become one edge conversion among several: MIDI's own
ticks, OSC's seconds or beats, MEI's `@dur` and dots, each done at that
protocol's edge and nowhere above it.

**Pitches carry their spelling.** `F#` and `Gb` are one MIDI number and two
different notes on the page, so the model stores what is written and computes
the sounding number from it. Going the other way is a *choice* and says so by
taking one: `Pitch::from_midi(midi, flats)`. This is what lets transposition be
musically right rather than merely audible — a major third up from C is E, not
F-flat — since an interval has a diatonic size as well as a chromatic one.

**What the model can hold, the emitter may refuse.** A tuplet, an accidental
past a double, more than one voice: each is representable and not yet writable,
and each refusal says which milestone owes the work, because a caller reading
"cannot" needs to know whether it is wrong or early. Silently snapping a triplet
onto the tick grid would produce a wrong score that looks right, which is the
one failure this layer must never produce.

## The score's operators: what each one had to decide

The operations over the score model are small, and each one had a musical
question in it that a plausible implementation gets wrong. The answers, because
none of them is recoverable from reading the code.

**Concatenation takes the first score's grid, continued — and refuses rather
than inventing a barline.** When the first score ends *on* a barline the second
score's meters are appended shifted by the measures the first occupies, so a 4/4
section before a 3/4 one is exactly that. When it ends mid-measure there is no
barline for the second grid to begin at, and a second score carrying a metric
layout of its own is refused. The alternatives were both worse: silently
re-barring the second score gives music in bars nobody wrote, and padding the
first to a full measure changes what the caller handed over. A refusal that says
which measure it stopped in leaves the choice where it belongs.

**Stacking refuses two different grids outright.** Two scores cannot share a
moment in time while disagreeing about where the barlines are, and adopting one
of the two would silently re-bar the other.

**Deleting and silencing are two verbs.** `delete` takes an item out and
everything after it moves earlier by its value; `silence` leaves a rest of the
same length, so nothing moves. One verb with a flag would have been smaller and
would have made the destructive reading the default reading — a caller who
wanted the second and got the first has a piece shorter than it was, with no
sign of where the time went.

**An edit names its item by id, not by index or by time.** An index moves the
moment anything before it is inserted or removed, so a caller holding one would
be addressing a different note after every edit but its own; a time is ambiguous
the moment two voices sound at once. Ids are minted on the sheet, travel through
every operation, and are re-minted only where two sheets meet — because two
items answering to one id would both answer to one edit, and the result of
joining two scores is a new score rather than either of its parts.

**An interval has two sizes, and both are stored.** Transposition takes a
chromatic size in semitones and a diatonic size in staff places; given only the
first, the second is the ordinary reading of that interval (4 semitones is a
major third, so 2 places). That is what makes a transposition come out spelled
the way it would be written — a major third up from C is E, never F-flat — and
it generalizes to inversion, which mirrors the notehead across the axis on the
staff and the sound across it in semitones, letting the accidental take up
whatever is left.

**Time-scaling leaves the grid alone, on purpose.** Stretching a phrase against
the barlines it was written on is what augmentation *is*: the values grow, the
bars do not, and the music re-bars and ties across them. An implementation that
scaled the grid too would keep the page looking identical and would have
performed no augmentation at all.

**Only three operations move both structures**, and they are the three that add
or remove time: `repeat`, `insert_measures`, `remove_measures`. Everything else
is content or grid, never both, which is the invariant the whole algebra is read
against.

## Accidentals: the engraver infers nothing, so the suppression is ours

The plan carried a hypothesis for years: that suppressing a redundant accidental
was "not a per-measure state machine but the difference between the *written*
accidental (`<accid>`, which forces the print) and the *sounding* one
(`@accid.ges`), which hands the decision to the engraver that already holds the
key signature". It also said to **verify before switching**. Verifying reversed
it.

Engraving one phrase both ways, and counting the sharps actually drawn:

| | `<accid accid="s"/>` | `@accid.ges="s"` |
|---|---|---|
| F sharp in C major | 1 — correct | **0 — a wrong score** |
| F sharp in G major | 2 — the armature's and a redundant one | 1 — the armature's only |
| the same F sharp twice in one bar | 2 | 1 (from the one written) |

Verovio draws exactly what the document says and infers nothing. `<accid>` is
always drawn, including where the key signature already implies it and where the
same note was altered earlier in the bar; `@accid.ges` is never drawn at all. So
an F sharp in C major written as the sounding form comes out as a plain F: a
wrong score that looks right, which is the one failure this layer must never
produce.

The decision is therefore the encoder's, and it needs **both** halves the
hypothesis denied — the key signature, and a per-measure memory of what has
already been printed. An accidental holds for the rest of its measure at its own
step and octave; a new measure starts again from the armature. Three things
print: an alteration the armature does not give, a **return to the natural** of a
step the armature alters (a sign, not silence — this is what the old encoder got
wrong, writing a C in F sharp major with nothing at all, which reads as C sharp),
and anything a caller marked as a courtesy accidental.

Two details the implementation settles. The memory is per **staff**, not per
voice: two voices share a bar, and the second does not restate what the first
printed. And only the **first piece** of a note split across a barline prints its
accidental — the tie carries it, and restating it reads as a second, different
alteration.

## A grand staff is one system because a barline says so, not because a gap is small

The playback cursor sweeps the vertical span of the system the sounding note is
on, and the span came from clustering the horizontal staff lines: a gap wider
than 500 page units started a new system. That threshold cannot tell a **grand
staff** from two systems — the two staves of a piano part are further apart than
the lines of one staff and closer than two systems, and any number between those
holds for one page size and not the next. It read a braced pair as two systems
and gave the cursor half the span it should sweep.

What settles it is already drawn: **a barline through the brace**. A single
vertical line running from the top staff's top line to the bottom staff's bottom
line exists only where two staves are barred together, which is exactly what
makes them one system. So the lines are grouped into staves by their own even
spacing — five lines, one gap, nothing to guess — and two staves are joined into
a system when a line is drawn through them. A page that wrapped into several
systems has no such line between them and stays several.

## A sounding length is not written onto the page

MEI has `@dur.ges`, a *gestural* duration, and it reads like the way to say
"written a quarter, sounds an eighth" — the exact separation the score model
keeps between the value on the page and the length in the air. Writing it turned
out to corrupt the performance rather than shade it.

Measuring the onsets an engraving produces, for four quarter notes where the
first carries the mark:

| | onsets (ms) | first note (ms) |
|---|---|---|
| plain | 0, 500, 1000, 1500 | 500 |
| `<artic artic="stacc"/>` alone | 0, 500, 1000, 1500 | 500 |
| `@dur.ges="8"` | **0, 250, 750, 1250** | 250 |

The engraver reads the gestural duration as the note's real one and advances its
own clock by it, so the note does not merely sound short — **every attack after
it moves a quarter-beat earlier and the measure comes out short**. A staccato
quarter is not an eighth: it sounds about half as long and the next attack is
exactly where it was.

So the emitter writes the articulation and **not** the sounding length. The model
keeps the fact (`Marks::sounding`), because it is a fact about the music and
because an interpreter can only honour what was kept; honouring it is the
interpreter's, which is the milestone after this one. The division is the
track's own: what is *written* belongs to the page, what is *heard* belongs to
whoever plays it, and putting performance into the document and asking a layout
engine's MIDI export to time it is exactly the confusion the two structures exist
to avoid.

The same measurement says a staccato alone changes nothing about playback today,
which is correct and is the interpreter's to fix, not the engraving's.

## A rest that fills a measure is `<mRest/>`

The emitter wrote every rest by decomposing its length into note values, which
for a whole measure gives `<rest dur="1"/>` — drawn hanging at the **start** of
the bar. A reader looks for a whole-measure rest in the **middle** of it, and MEI
has an element that says exactly that: `<mRest/>`, which an engraver centres.
The distinction is not cosmetic — a rest at the start of a bar reads as a rest on
the downbeat with something following it.

**The case that matters is a rest *longer* than a measure**, which is the
ordinary one rather than the exception: an empty staff under a written one is one
long rest, written by the operations that keep voices in step. Handling only a
rest that measures exactly one bar leaves that long one to the ordinary
decomposition, which writes a whole rest at the *start* of each bar it covers —
which is what this looked like on the page. So every full measure a rest covers
is written this way, and only its ragged ends, where it begins or finishes
mid-measure, are decomposed into values.

The glyph is the **whole rest**, hanging from the fourth line, which is the
convention for a full measure in any meter — a half rest means half a measure of
actual silence. The host has always listed `mRest` among the classes a click can
name, which is the earlier half of this layer expecting them.

## The interpretation is data, and the defaults claim as little as a player can

Reading a score back into events is not a conversion, so it is a layer with a
name (`clausters_core::notation::interp`) rather than a function on the emitter.
The reason is that no symbol can be read off the note it sits on: a dynamic
governs the notes **after** it, a hairpin is a shape over a stretch of them and
lives on neither end, a tie makes two items one sound. What can be read off one
note — a staccato — is exactly the case the previous decision showed must not be
written into the document.

Three choices in it were not obvious.

**Two lengths, always.** A performed note carries `dur` (written) and `sustain`
(heard), never one number. Collapsing them is the same defect `@dur.ges`
produced from the other direction: shorten the written value and every attack
after it moves. The pair maps onto the client `Event`'s own `dur`/`sustain`,
which already made the distinction, so nothing new had to be invented on either
side of the wire — the model and the event agreed all along and only the encoder
did not.

**The reading is a value the caller holds.** Every number it depends on — what a
staccato does to a length, what `mf` is in amplitude, how far a crescendo
travels, which positions in the bar are stressed — is one JSON object passed in,
and it comes *back* from the core (`clausters_core_interpretation`) so an
override starts from the defaults rather than restating them. Two symbols
therefore, not one: a client that wrote the dynamics table down for itself would
play the same score at a different amplitude than the other client does, and
nothing structural compares two tables. This is the same blindness `sheet_ops`
answers for the verbs, and the same answer.

**The defaults stress the downbeat and nothing else.** A player has to claim
*something* or it is not playing, and the smallest claim that is still a reading
is that a bar has a strong first beat — common to every metric style that has
any. "One and three of a 4/4" is a style, and a style says so by passing its own
accents; an accent may name the meter it applies in, since half a bar is a
different place in a 4/4 and in a 3/4. In the same spirit `detach` and `slur`
both default to `1.0`: a player who shortens every unslurred note is a reading,
not a fact, so the default reads the page literally and an interpretation that
disagrees is one key.

**What needed no rule at all.** A *tuplet* divides the time exactly in the
rational the item already holds, so onsets land on it without the interpreter
knowing tuplets exist. A *repeat* is not a symbol this model carries: repetition
is written out by the `repeat` operation, so by the time a sheet exists there is
nothing left to expand. Both are consequences of decisions taken two milestones
earlier, and are recorded here because "the player must expand a repeat" is the
kind of requirement that gets implemented before anybody checks whether the
model can state one.

**What the two directions lose is written down rather than found.** Events to a
score loses exact onsets (they snap to written values), continuous amplitude (it
becomes a dynamic or nothing), microtones and the instrument. Back again loses
the spelling, the stems and beams, which voice a note was in, and every mark
there is no rule for yet — a grace note, an ornament and a fermata are on the
page and read as ordinary notes. Pitch, written value and order survive both
ways, and that is the whole of what a round trip is entitled to assume.

## The score's edit path is the model's verbs, not the engraver's editor

`Score` drove verovio's editor: an edit was a `keyDown` action, a `commit` and a
reload. That worked and was a second implementation of what an edit to a score
means — one in the algebra every client binds, one reachable only from a process
with an engraver in it. The divergence this track exists to avoid, sitting in
the middle of it.

Now an edit is `Score::apply(&Op)`: the same payload the sheet verbs build,
applied to the model, re-emitted and reloaded. An edit to an open score and an
edit to a sheet in hand are the same operation through the same code, and a
standalone host holding a sheet performs it with no engraver anywhere.

**The engraver's editor stays, as the escape hatch it now honestly is.** A
document that could not be read into a model still draws and still plays, and
`transpose` falls back to `keyDown` for it — which is the one case where there
is no item to move. `Score::sheet()` is how a caller learns which of the two it
has, and a document with no model is a *state* rather than a failure.

**Dragging a note is `move_steps`, and it is not transposition.** Moving along
the staff takes the **key signature's** alteration for the letter it lands on:
drag a note onto a B in E flat and it is a B flat, because that is what reading
in a key means. `Op::Transpose` is the other thing — a named interval, keeping
the alteration the arithmetic implies. Two verbs because they are two acts, and
conflating them is how a drag in a flat key produces a natural nobody asked for.

**The model is re-read from the engraver after every edit, never trusted from
the operation's own result.** What the engraver holds is what the page draws and
what the next edit starts from; letting the two differ would mean applying an
operation to a score nobody is looking at. The same re-read runs after an undo,
a redo and a raw editor action, so the model is never behind the page.

**A reader has to apply the key signature, and finding that out cost a test.**
The emitter writes no accidental where the armature already gives one — correct
engraving — so a reader that took a bare `pname="b"` as B natural turned every B
flat in E flat into a B natural on the first save. The rule is the emitter's own
read backwards: start each measure from the armature, and let a printed
accidental hold to the end of that measure at its step and octave. Beside it, a
smaller one measured the same way: an accidental is an attribute on the note
*or* a child `<accid>` element, and our emitter writes the sounding one as a
child while verovio hands back attributes — so all four places are read, or an
alteration is lost depending on which side of the engraver a document came from.

## Note entry: the host names a place, and nothing more

*(The press described here is gone: note entry became a mode whose press names
the column it fell in, `"enter"`, and writes over what is there -- see "Note
entry is a mode, and an entry replaces". That the host names a place and
nothing more still holds.)*

The last gesture the score page owed, and the one that decides what a renderer
is allowed to know. A press on blank paper inside a staff reports
`"insert" <after> <position> <staff>` — the element the new note would follow on
that staff, how far up the staff the press landed, and which staff.

**It names a place and not a note**, which is the same division every other
score gesture keeps. A staff position becomes a pitch only once something knows
the clef and the key, and the host knows neither; a duration is not implied by a
click at all. Both stay the client's. What the host contributes is the one
measurement nobody else can make — where on the page the finger went — reported
in the ids the client engraved.

**It is its own opt-in (`entry`), not a second meaning for `editable`.** The
gesture takes over one that already does something useful: on every other page a
press on blank paper clears the selection. A page that had not asked for note
entry would start reporting an insertion every time a user dismissed one, which
is a regression disguised as a feature.

**The display list grew an `elements` list, and that is the widening `N3` asked
for.** To a renderer an id is an id: a staff's lines carry the staff's id, a
layer's carry the layer's, so "the nearest element to the left" was always a
staff line — and an insertion "after the staff" names nothing a model can
resolve. The engraving walk is the one place that knows which ids are sounding
elements (`is_element_class` already decides it, which is what makes a note's
parts stop having ids of their own), and it now writes that down instead of
throwing it away. It is a list of ids rather than a flag on each primitive
because one id covers the several primitives a note is drawn from.

**Reading the place is `pitch_at`, in the core.** A clef puts one pitch on one
line, the top line is two diatonic steps above the fourth, and the arrival takes
the key signature's alteration — so clicking the middle line in E flat writes a
B flat. A client working that out for itself would be a second answer to a
question the engraver already answers, and the two would disagree the first time
somebody wrote a C clef. It reaches the wire as an optional `position` on the
`insert` verb, which costs no ABI: the operation was already data.

## What an event says about a page: three classes, and only one of them fits on a note

The forward path — a client's own events engraved as a score — read `midinote`
and `dur` and nothing else, so a phrase that had been shaped in a client arrived
on the page as bare noteheads. The question it left open was where a client
*says* notation intent, and it looked like a choice between three designs. It is
not: the three are carriers for three different classes of data, and which
carrier a thing takes follows from what the thing is.

- **Per-note intent** — an articulation, a dynamic, an ornament, a grace note,
  a spelling — is one note's, so it rides the **slot**, which is symmetric: the
  same field is written going out and read coming back.
- **Spans and document-level material** — a slur, a hairpin, a tuplet group, the
  meter, the barlines, the title — cannot live on an event at all, because they
  have two ends or none. They travel *beside* the voice, as the model's grid,
  spanners and header, and the model's own verbs write them.
- **Bindings** — a staff to an instrument, a voice to a channel — are neither,
  and the interpreter already settled them: a page does not say what plays it.

So the slot grew nine optional fields and the wire form grew nothing else. What
makes that safe rather than a widening is the rule every field obeys: **a
musical fact, not an instruction to the encoder.** `articulations: ["stacc"]`
and not `shorten_drawn_value`; `accidental: "written"` and not `print_accid`. A
fact can be read back by the interpreter that plays the page; an instruction can
only ever be written, and a format that accumulates instructions stops being a
score.

**The slot stopped being an untagged enum.** It was two variants with no
discriminator — a slot with no pitches *is* a rest — and that stays true, but as
a struct with a defaulted pitch list rather than a fallthrough. With nine
optional fields the enum had a trap: a note whose `articulations` was misspelt
or mistyped failed the note variant, matched the rest variant (which ignores
unknown keys), and became a **silent rest**. The struct refuses unknown fields
instead, so a typo is an error with a name.

## A sustain reaches the page only where no symbol already says it

An event carries `sustain`, the model carries `Marks::sounding`, and the two are
the same quantity, so the reduction that engraves an event looks like it should
copy one into the other. It must not, in two cases, and both are about not
counting the same fact twice.

**A note held for its written value says nothing**, so no length is written.
The client's default `legato` of 0.8 shortens every note ever sequenced; it is a
playback default, not a statement about the music, and a score that recorded it
would carry a sounding length on every single note. Only a `sustain` the caller
wrote reaches the page at all.

**A note that is both staccato and short is one fact, not two.** The staccato is
what the writer said; the short length is what an interpretation made of it. Put
on the page as both, the next reading would apply the staccato to an already
shortened value and the note would halve again on every round trip. So
`sounding` is *what the sustain says that no symbol said* — and where a symbol
said it, the symbol is written and the length is dropped.

This is what makes the round trip stable rather than merely lossy in a
documented way. A score read into events and written back engraves the same
page, because what comes back on the event is what was **written** — the
articulation verbatim — and not what the interpreter computed from it. The
interpreter's note carries both for that reason: `sustain`, which is the
performance, and `marks`, which is the page.

## Note entry is a mode, and an entry replaces

The score editor's first way of writing was a press on blank paper that
inserted a note after the element to its left (`notation::edit::insert`, the
`"insert"` gesture, above). It was taken out of the page because it is the
wrong act for writing into finished music: an insertion adds time, so a note
written into a bar pushed every note after it along and re-barred the ones
after that, and writing and selecting were one press told apart by a pixel.

**Note entry is a mode with an edit cursor**, the settled principle of notation
programs. Outside it a press selects and a drag moves; inside it the cursor
stands on a staff, in a voice, at a time, and what is entered is written
**over** the stretch it covers (`Op::Enter`): nothing after it moves, in the bar
or past it, a value longer than what is left of the bar goes on tied, a voice
that ends early is padded and a voice the staff does not have is made -- which
is also how a second voice is written against a first. The window opens outside
the mode, and playing leaves it. `insert` stays, as its own verb, for what adds
time.

**The page names a column, not a neighbour.** In note entry a press reports
`"enter" <at> <position> <staff>`: the sounding element whose column the press
fell in, read as the time it starts at (`Op::Enter`'s `item`), rather than the
element the new note would follow. A press on a note of the cursor's voice adds
the pitch to its chord, which is the other way a chord is built beside Shift
and a letter.

**A mode's keys are a scope of the key table.** The letters are pitches inside
note entry and bindings of the host's elsewhere (`E` is `split`), so a chord is
not one verb across the whole host any more: a window names the scopes in force
in it (`keys`), and a scope's chords are read first. That is what keeps every
key of the mode a binding a user can change in a file (`[gui.keys.note_entry]`)
rather than a letter spelled in an element. Escape, the dismissal, may be bound
in a scope and nowhere else, since leaving a mode is a dismissal; and it now
reaches the focused element before the front's own close, which a text being
typed over on the page had needed and never had.

## A page break is honoured by laying the score out twice

The engraver turns a page where the paper is full and nowhere else: its cast
off reads a system break and has no visit for a page break, which only its
`encoded` mode honours -- the mode that breaks nowhere it was not told to. That
is the engraver's design, and patching it was ruled out (the user, 2026-10-05).

**So a score with written page breaks is laid out twice** (`Score::pages`,
2026-11-09), every decision the engraver's. First each run of pages -- the
measures from one page break to the next -- is laid out as a document of its
own (`notation::sheet_to_mei_pages`), only to read where the engraver breaks
it: the first measure of each system on each page. Then the one document is
written with all of those breaks, a page break before each page and a system
break before each other system (`notation::sheet_to_mei_paged`), and laid out
in `encoded` mode. That layout is the drawing: one document, so a line across
a page turn is drawn open at the edge on both sides as the engraver draws one
across a system, a tie over the turn is two halves, and every page carries its
running elements -- the first page's head, and the page number on every other.
The runs' own writing (a line cut at each run's edges, the head of a run that
does not start the score) is read for its breaks and never drawn.

Until then the runs were the drawing, and across them a tie was dropped, a
glissando was not drawn and every other line was closed at the run's edge. A
selection of the one document was tried first: it draws the lines right, but
a selected page has no running elements (no title, no page numbers), so it
was left. What the two layouts cost: a score with page breaks is laid out once
per run and once more whole on every edit; and the last system before a page
break is justified to the full width, which a run laid out alone did not do,
since `encoded` mode justifies every system but the document's last.

**A line through a measure drawn as a repeat** is written at its beat of the
measure: the sign stands where the item's element would be, so a line or a
mark at that item is written at its beat, and the reader puts it back on the
item the measure holds there -- which is the repeated measure's copy of it,
since reading a repeat makes its items again.

## What a page writes is what it plays, and the cursor follows the reading

The score's model grew the elements the engraver draws and the model did not
hold -- tremolos, a rolled chord, a glissando, breaths, let it ring, the octave
lines, the pedal, phrase marks, brackets, beams across a barline, tempo marks,
directions, rehearsal marks, fingerings, chord symbols, lyrics, changes of key
and clef, endings, navigation marks, measure and beat repeats, numbered rests,
a staff's lines, names and transposition, and the groups of staves. **Each is
notation and sound** (the user, 2026-10-05): it is written, read back, edited by
a verb of the palettes, and heard.

**A page is played in another order than it is read.** The interpreter plays
the repeats, the endings and the jumps out (`notation::unroll`), and every item
is heard once a pass; a tempo mark is the tempo the sequence's map carries from
where it is heard; the pedal is controller 64; an octave line and a transposing
staff move the pitch; a tremolo is its repeated notes; a rolled chord is
staggered; a glissando is a bend over the note, which takes a sequence to MIDI
2.0, since a MIDI 1.0 channel cannot bend one note (`notation::performance`).
That retires the earlier rule that a repeat is written out before a sheet
exists.

**The page's cursor is drawn over the same reading.** It used to be drawn over
the engraver's timemap, and the playback took the engraver's tempo to agree
with it; the engraver knows no repeat, so a score with one would play its
second pass under a cursor standing still. A page written from the model is now
timed by the model's own performance -- its items, the parts a barline splits,
a chord's notes and the rests, each where it is heard -- and the engraver's
timemap is kept for what only it knows, the MIDI values of what it drew.

Two spellings were chosen against MEI's grain, for the round trip every edit
makes through the engraver's normalized document. A two-note tremolo is two
items of one value each, written inside `<fTrem>` with the value of the two
together and halved again when read. Whether empty measures are drawn as
numbered rests is the section's `type`, so it survives a score that has no run
to show it.

## A symbolic score executes in the host (2026-10-31)

*Decided by the user 2026-10-31, on the choice the applications' port left
open.* The applications run in the host and both clients are their handlers,
and the record also said the engraver lives on the client and never in the host.
Both could not hold for the score editor: an application cannot lay out a page
through a port only its client has, and a standalone host has no client to ask.
Three shapes were weighed. Keeping the score where the engraver was keeps the
rule and leaves coordination and playback written twice, in Python and in
TypeScript, for one editor among five. Having the host ask a client for every
engraving keeps the rule too, at a round trip per edit and with nothing for a
standalone host. **The score goes with the others**: its document, its engraver
and its playback are the host application's, and `Score` in a client is a remote
handle while a window holds it, as a `Multitrack` is.

The engraver stays a port (`notation::AnyEngraver`), so the crate still links
none. Natively the host links libverovio, as `--score` already did. In a page it
is the same pinned verovio in wasm, in a worker the application's thread asks
synchronously over the shared memory, the way that thread reads the page's
storage; it loads on first use, so a page that never engraves never fetches it.
A host built without an engraver refuses the open and says why. A client keeps
the engraver it has for what opens no window (`engrave`, a `Score` read and
written from a script).

**The roll goes with it**, which the plan had left implicit. A roll and a page
over one sequence are one history, and one history needs one `Editing`: with the
page in the host and the roll in the client the order would split. So a client's
`edit(sequence)` opens the host's roll too, its sequence a remote handle while a
window has it. Three things a client-side roll had change with that, knowingly:
it plays on the application's engine rather than the client's server, so a def
sent to that server does not sound there (written down in the plan's Found by
use); each window has a transport of its own, as the plan asks of views over a
sequence, where two rolls used to share one; and a roll's undo order goes with
its last window, as a multitrack's does, where a client's order outlived it. A
roll is no longer composed into a client's own window (`extra`, `context`):
that stays the points editor's, which edits the client's curve.

## A tie holds the pitches two items share (2026-11-09)

*Taken in `O37` (`crates/clausters-document/PLAN.md`).* A part written with
ties is one voice of chords, every note cut where another starts or ends, so
a tie there holds some notes of a chord and not others. The model's tie was
the item's (`Item::Note.tie`), and the interpreter and the MEI writer read it
as tying the whole item into the next: a note tied into a chord silenced the
chord's other notes, and a chord tied into one note held them all on. MEI
ties note by note, and so does the page.

The rule now is that **a tie holds the pitches the two items share** and
nothing else (`Item::holds`), which is what a page means by it and what
`settle_ties` already kept a tie by. One case is still out of reach of a flag
on the item: a pitch held under the same pitch repeated, where both items have
it and the tie must not hold it. For that one the tied item names the pitches
it holds (`Marks.tied`), empty in every other case, so nothing saved changes
its shape. What plays differently is a tie into a chord or out of one, which
now attacks or lets go the notes the two do not share, as the page says.

The other shape was a tie flag on each `Pitch`, which is MEI's. It was not
taken because a pitch is a value compared and copied everywhere a spelling is
(more than sixty places build one), and a pitch that differs by whether it is
tied stops being the same note to every comparison that does not care, which
is all of them but this one.

## Verovio is followed, not adapted (2026-11-09)

*Decided by the user, after measuring what an edit costs the score editor and
reading where upstream is going.* This project **uses verovio as it is and
draws what it engraves on the GPU**: the engraver lays the page out, the walk
turns its SVG into a display list, and the host renders that list with WebGL
or its native GPU backend. Nothing here optimizes verovio's work or replaces a
part of its logic with one of our own, and its development is followed: the
pin moves to each release (`third_party/verovio.pin`), native and browser
together, and what a release changes in its output is adapted to in the walk
(6.4 added a hidden placeholder per scoreDef inside a system, which its own
style sheet hides and the walk now skips).

**What was weighed and left.** Engraving a score by parts was measured and it
works: a system laid out from a document holding only its measures and their
neighbours, selected with one system on either side, is primitive for
primitive the system of the whole layout, at about 8 ms against 120 ms. It was
not taken, because it rests on behaviour verovio does not promise (that a
system depends only on its first measure, that a selected system lays out as
the whole one does, that a slur's curve depends only on the measures it
spans), it would rewrite the score's document as a set of fragments of our
making, and upstream is building the same thing its own way: its editor lays
out again only the range of pages an edit reaches (`Doc::SetFocus`,
`PageRange`), and its edit vocabulary grows release by release. If incremental
engraving comes, it comes through verovio.

**What it means for a line across a turn.** Verovio draws every line that
crosses a system or a page turn by the engraving convention (a slur as two
whole curves open at the edge, a tie as halves, a hairpin left partly open, an
octave line restated as `(8)`, a bracket without hooks, a glissando at half its
slope), and it does the same at the edge of a selection of measures. So where
this project cuts a score into documents of its own, as the page view does at
a written page break, the cut is ours to remove, not ours to patch: the page
view now lays the one document out in verovio's `encoded` mode with the breaks
verovio chose ("A page break is honoured by laying the score out twice").

## A page's notes are the model's reading (2026-11-09)

*Taken in `N10` (`crates/clausters-gui/PLAN.md`).* Every page a score produces
carries the notes that sound, and they were asked of the engraver one
element at a time, each answer a search of the whole document: 1.25 s of
the 1.78 s an edit cost at five thousand notes. The cursors were already
read from the model's performance, so the page and its sound came from two
readings. Where the document was written from the model, the notes are now
read from the same performance as the cursors, and named as the page draws
them (a chord's pitches by `Note.written`). A document from elsewhere has
no model and is still played as the engraver plays it.

Comparing the two readings found the engraver's was not what the page
writes in three places: it attacks the continuation of a tie again, plays
the notes under an octave line as written, and holds a staccato for its
whole value. The model's reading holds the tie, hears the octave and
shortens the staccato, which is what the score plays everywhere else.

## A start repeat is the left barline of the measure it opens (2026-11-09)

The model kept every barline as a measure's right one, a start repeat
included: the barline after the measure. That cannot say a repeat that opens
the score, which has no measure before it, and the engraver draws a right
barline at the end of its system, so a repeat opening a system was drawn at
the end of the one before -- it splits only a double repeat at a system break.
MEI and the engraver write a start repeat as `left="rptstart"` on the measure
it opens. So the grid's `(measure, "rptstart")` now names the measure a repeat
**opens at**, and is written as its left barline; every other barline stays
the measure's right one, and a measure may hold both. A document that writes
the start repeat on the right of the measure before is read as opening the
next one, which is what it draws. Found opening a score exported by a notation
program, whose opening repeat was missing.
