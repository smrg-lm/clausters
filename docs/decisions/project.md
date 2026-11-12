# Build, release and the project's conventions

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Source files are ASCII; the books are not

Code, comments, doc comments, docstrings and the strings an application prints
carry no character above U+007F. The sweep that made it true (2026-09-19)
replaced about 13 000 of them: 12 736 em dashes, then the arrows, ellipses,
middle dots, minus signs, square roots, superscripts, subscripts and Greek that
formulas in comments had reached for. `tests/ascii.rs` keeps it that way.

**Why not everywhere.** The books keep their typography, and the split is not
taste. A book is read **rendered** — mdBook has already chosen the font and the
width, and an em dash there is a typeset em dash. A source file is read in
whatever the reader has: a terminal at an unknown encoding, a diff in a
pager, a `grep` pattern typed by hand, an editor whose font has no glyph for
U+2212 and draws a box. The character costs the reader something and repays it
only in a medium the source file never reaches. A docstring is the interesting
case and it lands on the source side: it is published into a book, but it is
read in the editor first, by whoever is changing the function.

**Where the character stays.** Not every non-ASCII character in a source file
is typography — in some of them it is the data: the font's glyph table is
keyed by the character each arm draws; the dead-key composition docs name the
marks they compose; a caret test needs a letter that takes two bytes; the OSC
utf8 vector exists to carry one. Those files are listed **one by one, with the
reason**, in the test's `DATA`, and a second test fails if a listed file turns
out to be all ASCII after all — so the list cannot quietly become a place
things are parked. Where a glyph must reach a screen from otherwise ordinary
code, it is written as an escape (`"\u23FB power"`), which the web client's own
HTML was already doing with `&#9211;`.

**Why a test and not a habit.** The em dash had been the house style in source
files for the project's whole life, and the same day this was decided, one
change had been committed with both spellings in it — the mark is invisible
until someone looks for it, and no formatter, linter or compiler reads prose.
A rule about characters is exactly the kind a test can hold and a person
cannot.

## Package SemVer is decoupled from the binary ABI counters

Compatibility is tracked by **two monotonic integer counters** —
`ABI_VERSION` (the shm segment layout + embed C ABI) and `CORE_ABI_VERSION`
(the core FFI surface) — not by the package's SemVer. A release bumps a counter
only when *that* boundary changes incompatibly, and both are advertised by
runtime calls (`clausters_abi_version()`, `clausters_core_abi_version()`) that a
peer checks on attach/load, refusing to connect on a mismatch.

**Why.** A binary boundary needs a check that an *already-compiled* peer can make
at runtime, before it trusts a single byte of layout — SemVer strings cannot do
that job, and the two boundaries evolve on independent cadences (by 0.11.0 the
core FFI is at v68 while the embed/IPC ABI is at v11). A monotonic integer per
boundary is exactly the scsynth plugin-ABI lesson: every binary seam is
versioned and verified where it is crossed. SemVer is left to govern the
*package* — what `cargo`/`pip` resolves — where it belongs.

**Where the package's number is written** is a separate question, and it had the
same shape as the drift this section exists to prevent: one number, ten files,
one checked pair. The crates inherit `[workspace.package].version` now, the five
manifests that cannot inherit anything are written by `scripts/set-version.sh`,
and `tests/versions.rs` fails when any of them disagrees — including when a
crate writes its own number instead of inheriting, which is how the next one
would drift. The two ABI counters are deliberately *not* in that machinery: they
are not the package's number and do not move with it.

The one **linkage** rule keeps them from drifting into contradiction: a release
that bumps either counter must also bump SemVer's breaking tier (the minor while
the major is `0`, per standard pre-1.0 SemVer where the minor acts as the major;
the major once at `1.0`). The reverse is not required — a minor can ship purely
additive source-API work without touching either counter.

**Amended 2026-09-19: no number here is a distance, and none of the three is
the last tag's plus one.** This entry used to argue that a counter measures
"distance, not history" — that a boundary changing twice before shipping should
*amend* the first bump rather than add another — and the release rules grew a
matching clause for SemVer, that the breaking tier moves exactly once per
unreleased cycle. Both halves were wrong, in the same way and for a reason each:

- **The counters are read by equality and nothing else** (`header.abi_version
  != ABI_VERSION`, `got != CORE_ABI_VERSION`). Nothing subtracts, orders or
  counts them, so "distance" was a quantity with no consumer. Amending instead
  of bumping also costs something real in this checkout, where the staged
  `_bin`/`_libs` copy wins over `target/`: a counter that moved turns a stale
  staging into *"speaks ABI v21, this binding v22"* on attach, while under the
  amend rule both sides read the same number all cycle and the same staleness
  surfaces as a missing symbol at some later call site.
- **A number never tagged is not a release skipped.** The argument against
  passing through versions was that it "tells about releases that never
  existed" — but an invented release is a second *tag*, not a number the tree
  passed through. The version in the tree is the **development version**: what
  a tag would publish if one were cut today. Nobody resolved, installed or
  depended on `0.9.0` or `0.10.0`, so nothing was burnt by them; `0.11.0` is
  simply the development version after them, moved for the volume of change
  since `v0.8.1` and not for any boundary. What has to hold is that the
  sequence increases and never reuses a number already tagged.

So all three numbers are checked the same way — **that they differ** from what
the last tag carried — which is exactly what the linkage rule above triggers on,
and all any consumer reads. What the amend rule was really defending is the
sentence that opens this entry: the seam is described by where it *is*, not by
how it got there. That survives; the arithmetic it was dressed in does not.

The mechanical release rules live in the `release-versioning` skill; this entry
is the *why*.

## Docstrings are Google-style Markdown: markup a reader has to skip is a cost

Python docstrings are read twice, and the *first* read is the one that decides
this: in the source, by whoever is editing the function. Sphinx/RST field lists
and roles (`:param x:`, or `:class:` wrapped around a symbol) pay for their
structure in characters
that a human has to look past on every read — colons, role names, nested
backticks around what is otherwise an ordinary sentence. Google-style sections
plus plain backticks say the same thing with markup that reads as prose. So the
convention is **Markdown, Google style, no RST** — the rule that keeps
`clients/python/docs/build.sh` and the rustdoc honest is a consequence, not the
reason.

The toolchain agrees, which is why the choice costs nothing: nothing in the
chain speaks RST. The two books are mdBooks, the client's API page is generated
by pydoc-markdown (a static AST parse emitting Markdown), the Rust reference is
rustdoc. An RST role here is not "less supported" — it is inert text that lands
verbatim in the published page.

**Cross-references fall under the same rule, and cost more than they look.** A
reference only means something after a resolver has run: it needs a symbol table
plus a URL scheme for the output. Move a symbol or change generators and it
breaks — which is why the two books deliberately cross-link by their Read the
Docs URLs rather than by symbol resolution.

The consequence is sharper when the reference sigil is a character that occurs
in ordinary prose. pydoc-markdown's own `#name` syntax (neither Markdown nor
RST, a third thing) is matched by `\B\#`, so the quote in `"#rrggbb[aa]"`
satisfied it; nothing resolved, and the fallback **rewrote the text** — dropping
the `#` and injecting backticks inside an already-open code span — so the
published reference documented the color format as `"rrggbb[aa]"` in all four
places it appeared. The source was fine; only the artifact was wrong, behind one
build warning. `#` is everywhere in this domain (hex colors, shell comments, the
C preprocessor, anchors, issue numbers), so the collision was a matter of time.
The `crossref` processor is therefore **not** in `pydoc-markdown.yml`: removing
it changed exactly those four lines out of a 351 KB page, so nothing was relying
on it. The general form: a docstring should mean the same thing read raw,
through `help()`, or rendered — and a syntax that only resolves in one of those
does not merely fail to link when it misfires, it edits your prose.

## Publishing is a step, and a checker stands in for rehearsing it

The npm package is built, tested and installable from a checkout long before it
is published, and the publication itself is deliberately a later step. The risk
that creates is that publishing is the one operation nobody rehearses: its
mistakes — a tarball carrying the emitted modules but not the wasm bundles
`build.sh` stages beside them, a `version` that drifted from the crate's, an
`exports` entry pointing at a file the `files` list leaves out — are invisible
here and only surface in somebody else's install.

So the check runs now, twice: `tools/check-package.mjs` is what `prepublishOnly`
runs, and `tests/package.test.ts` runs it in the ordinary suite along with a
read of what `npm pack --dry-run` would actually ship. The version rule it
enforces is the repository's — package, crate and wheel are one release, one
SemVer — while the binary ABI counters stay separate, as they are everywhere
else.

## A cost gate compares two runs, not a number, and its threshold is measured

The R track's invariant is that a refactor closes with a before/after
`crates/clausters-server/examples/bench.rs`. That is a step a human remembers, and a check that depends
on someone remembering is one that eventually is not run — so it became a CI
job. What the job is *not* is the obvious design.

**Committing a baseline number does not work.** `ubuntu-latest` runners are
shared and virtualized; 10–20% swings between runs of identical code are
ordinary. An absolute threshold against a stored number fires on the weather,
and a gate that cries wolf is ignored, which is worse than no gate. So the job
builds and benches the **merge base** and then the head *in the same job on the
same runner, back to back*, and compares those two. Both measurements share the
machine, the thermal state and the noise; the ratio means what the absolute
values do not. The cost is one extra release build per pull request.

**The thresholds are measured, not chosen, and they differ per metric** —
because the metrics do. Three runs of an identical build, compared pairwise on
this machine:

| metric | median | worst |
|---|---|---|
| `x_real_time` (210 comparisons) | 0.5% | 5.6% |
| `peak_block`, aligned, ≥32 voices | | 21.0% |
| `peak_block`, staggered, ≥32 voices | | 34.6% |
| `peak_block`, one voice | | **251.8%** |

So throughput is gated at 10% and the peak block at 50%, each roughly twice its
observed worst case: loose enough never to fire on noise, tight enough to catch
the cliff the gate exists for — a lost inline, an allocation that crept into the
block path. A 3% drift is invisible to it on purpose.

The last two rows are why some measurements are **reported and never gated**.
The spectral peak at one voice is a single hop's spike, and two runs of the same
build differed by 250%: it is not a measurement. The *staggered* peak is worse
in a more interesting way — it measures whether two chains' hops happened to
collide this run, which is exactly what the stagger exists to scatter, so its
variance is the feature working. The **aligned** peak is the deliberate worst
arrangement (every chain hops on the same block), which makes it a property of
the code, and it is the one that is gated. The Faust rows carry whatever the
LLVM JIT decided that run, and the worker sweep reads the machine's core count;
both are printed and neither can fail a build.

**A gate nobody has seen trip is a gate nobody knows is wired up**, so it was
verified in both directions before landing: three identical runs pass in all
three pairings, and `#[inline(never)]` on two hot bus accessors turns eleven
gated rows red at once (`default/1` −10.8%, `sine/ugen/1` −16.2%, the aligned
512-voice peak −49.5%).

## The two vendored wasm artifacts are built by the release, from three pins

*2026-08-27.*

`libfaust-wasm` and `verovio.wasm` — the Faust compiler a page JITs a def with,
the engraver it lays a score out with — are the only things a page loads that are
not compiled from this repository's sources. Both were built on a maintainer's
machine and staged by `clients/web/build.sh`, which left the packaging half open:
CI grows an Emscripten leg, or the artifact is fetched from somewhere and pinned
by digest.

**Fetching was never actually available**, and that is what settles it. There is
a published build of each, and neither is the one we need: npm's `verovio` is a
different build of the same version, and `@grame/faustwasm` binds the Faust
*source* API only — not the Box and Signal APIs, which `third_party/faust-wasm-bindings.patch`
adds so that the one JSON interpreter (`faust::boxes`, `faust::signals`, in Rust)
drives the same compiler in a tab as in a window. A def built with the box API
would compile in a window and fail in a tab. So the artifact is ours, somebody
has to build it, and "pinned by digest" would only ever pin *our own* build —
which is the pin file, not a digest.

**The cost of leaving it on a laptop was measured and it was not hypothetical.**
`package.json`'s `files` lists `dist/`, and what is not in `dist/` is not
published: `publish-npm` built the package in a runner with no SDK, `build.sh`
printed its two `note: vendor/… missing` lines, and the published package shipped
**with no compiler and no engraver**. It installs, it loads, it plays — and then a
FaustDef will not compile and a score will not engrave, on the user's machine.
That is the quietest failure in the package.

So: **the release builds them**, through `.github/actions/wasm-vendor`, the same
restore-or-build composite already proven for the two native libraries, running
the same vendored recipes a maintainer runs. And **`tools/check-package.mjs`
requires them**, so the silence cannot come back: a tree without them is not
publishable, while a tree without them is still perfectly developable — nothing in
`src/`, the suites or the smokes reaches either artifact.

Two consequences worth naming. **The SDK is pinned** (`third_party/emsdk.pin`,
read by both recipes and by the composite's cache key): these artifacts are almost
entirely toolchain output, so an unpinned `emsdk install latest` meant the
published package was built by whatever resolved that morning — and two of the
five departures `build-faust-wasm.sh` documents exist precisely because a current
emcc changed under the pinned Faust's build files. **CI does not build them**: the
two builds are ~20 minutes cold, `ci.yml`'s smokes reach neither, and a page that
wants the compiler is a manual test either way. The cache key is the three pins
plus the recipes' hashes, so a repin or a flag change moves it with nothing to
keep in sync in the callers.

## The work has no name, so the prose names the structure

*2026-09-17.*

"Piece" named no structure and stood for whichever one a module happened to
hold, some four thousand times across Rust, Python and TypeScript: a
`Multitrack` in the projection and the editors, the transport's axis in the
server and the shm segment, a tempo map's owner in the clocks, a fragment of a
split note in the notation model, and the work in general in the books. A
reader had to decide, every time, which of those was meant -- and the code did
too, which is how `HeadClock::Piece` came to name the transport and
`edit::Piece` the multitrack in the same tree.

The rule is the one CLAUDE.md already stated for "material": **name the
structure**. So the word is retired, in both senses. "The piece" is not a term
of art here, and neither is "the composition": no structure is the work, so the
prose names a `Multitrack`, a `Timeline`, a document, a session, a take, a
score, a pass, a part. A sentence that resists the substitution is a sentence
that did not know which one it meant, which is what the rename was for -- and
several did not: the `PlayheadSync` modules alone used the one word for the
transport, the pass in flight and the multitrack a playback sounds, three
paragraphs apart.

**What moved on the wire**, since a word a client types is not prose:
`/gui_headClock` takes `"transport"` where it took `"piece"` (with `--clock
transport`, and `--session` implying it); the editing request and outcome key
`piece` is `multitrack`; the mixer's graph is `clausters.multitrack.N`; the
transport row's controls are `transport_rewind`, `transport_play`,
`transport_stop`, `transport_clock`. The recorded vectors are regenerated with
them.

**The record keeps the old word.** `docs/history/build-log.md`, the `PLAN.md`
set and the entries of this file above were written when it meant something,
and a record that is rewritten is not one -- as with `buy`, which survives in
the plans for the same reason. Everything a reader types against, and every
page explaining it, uses the structure's name.

Plain English survives: one piece of code, a turn metered in pieces, a file
written in two pieces. What is retired is the *term of art*.

## The toolchain is pinned, and a new compiler arrives as a commit (2026-11-11)

**Context.** CI installed whatever `stable` was that day and the repository
had no `rust-toolchain.toml`. Every rustc release could therefore turn a green
tree red with no code change: a new lint, or an existing one widened, landed
on code nobody had touched, in whichever push happened to come next. The rule
that answered it was a discipline (expect warnings you did not cause, clear
them in a commit of their own), and it put the cost of a release on whoever
pushed first after it.

**Decision** (the user's, 2026-11-11).

- **One file names the toolchain**, `rust-toolchain.toml` at the root: the
  channel, the components the checks need and the wasm target. rustup reads
  it from any directory under the root, so the workspace, the GUI host, the
  editor application and the fuzzer build with the same compiler, on a
  developer's machine and in CI. The audit harness already pinned the same
  version for the same reason.
- **No workflow names a version, a component or a target.** A composite
  action installs what the file says and makes it rustup's default, which
  matters to the cost gate: it builds the merge base in the same job, and a
  base that predates the pin has no file to read.
- **The newest stable is tried weekly**, by a job that runs fmt, clippy and
  the libfaust-free tests with the pin overridden. It runs on a schedule and
  on demand, never on a push, so it cannot fail a change that did nothing
  wrong. When it goes red the pin is raised and what the new compiler reports
  is fixed in that commit.

**What it does not cover.** The web package's host with the applications in
it needs a nightly and keeps naming its own. A distribution's own Rust has
to match the pinned version, which `BUILD.md` says.

**The release profile was measured and left alone.** Thin link-time
optimization with one codegen unit, over the 88 gated rows of
`crates/clausters-server/examples/bench.rs` on one machine, two runs each way: the median throughput
moved by +0.8% and 0.0% against a run-to-run median of +2.9% between two
builds of the unchanged profile, with the same spread at both ends (about 8%
either way). It also made the release build a third slower. The dev profile
does change: line tables only, which made the tests of two crates 39%
smaller on disk and keeps file and line in a backtrace.

## The workspace is virtual, and the server is its default member (2026-11-12)

The root manifest was the workspace and the server's package at once. That
was the project's first shape, a server with a client beside it, and it
stopped being true: the server is one component among the document, the
applications, the GUI host and the clients, and nobody enters an application
through it. A tree whose root is the server's `src/` says otherwise to
whoever opens it, so the server is `crates/clausters-server` and the root
holds the workspace alone.

**The package is still `clausters`.** The directory takes the component's
name, the one the documentation already uses for the Rust server; the
package, the library and the binary keep theirs, so no `use clausters::` and
no command a user types changed.

**`default-members` is the server, and that is what keeps the commands.** A
virtual workspace with none makes `cargo build` and `cargo test` act on every
member, and cargo unifies features across what one invocation builds: the
GUI host turns `realtime` and `notation` on, which the def-family rows build
without. With the server as the one default member, a command with no `-p`
means what it meant when the server was the root package, in CI, in the
release and in the feature matrix alike.

**What a build gains is nothing, and that was the first verdict.** One
lockfile and one `target/` were already shared. The review that left this
out measured only that; what it changes is what the tree says.
