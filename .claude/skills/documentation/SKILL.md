---
name: documentation
description: How to write and place any Clausters documentation — first the user-vs-development split, then Diataxis as an internal lens for the user docs (tutorial / explanation / reference; how-to only on explicit request) across the two mdBooks, the generated API references (rustdoc + pydoc-markdown), and the development docs (architecture as C4, contributing as governance, ADR-style records in docs/decisions.md, roadmaps in the PLAN.md set). Consult before adding, moving or restructuring any doc page, docstring or example.
---

# Documentation: user vs development, then Diataxis

Classify every page on one axis **first: who is it for?**

- **User / consumer documentation** — for people who *use* Clausters from the
  outside: drive the server over OSC, write a client, or embed the crate through
  its public API/ABI. They never read the source. Diataxis structures these.
- **Development documentation** — for people who *work on* Clausters itself: the
  engine, UGens, threads, the ABI implementation, and the decisions behind them.

The test: if the page is about Clausters' own internals *as something you would
change*, it is development; if it only needs the public surface, it is user. The
gray zone — the feature pages, `ipc.md`, `using-as-a-library.md` — explains
internals *to inform use*; those stay user docs, but keep the usage contract
(commands, signatures) crisp and leave the real implementation story to
`architecture.md` and the rustdoc.

Decide the audience, then (for user docs) the Diataxis quadrant, *before*
writing. Then place the page.

## The doc map (where things live)

- **Server / workspace book** — mdBook in `docs/` (`docs/SUMMARY.md` is the ToC,
  the repo-root `book.toml` the config, `README.md` the front door). Build with
  `mdbook build .`; output `book/` is git-ignored. Reuses the `docs/*.md` in
  place.
- **Python client book** — mdBook in `clients/python/docs/` (its own `book.toml`
  and `src/`). Build with `clients/python/docs/build.sh`.
- **Web client book** — mdBook in `clients/web/docs/` (its own `book.toml` and
  `src/`). Build with `clients/web/docs/build.sh`. The **thinnest** of the
  three by design: it documents what the browser makes different (the two
  carriers, promises everywhere, components in a document) and links the other
  two books for the shared model rather than restating it.
- **Rust API reference** — the rustdoc (`cargo doc --no-deps`), generated from
  `///` / `//!` doc comments. This *is* the crate reference; no hand-written
  mirror.
- **Python API reference** — `clients/python/docs/src/api.md`, **generated** from
  the package docstrings by pydoc-markdown (`clients/python/pydoc-markdown.yml`;
  both it and `book/` git-ignored). Never hand-edit `api.md`.
- **TypeScript API reference** — `clients/web/docs/src/api/`, **generated** from
  the sources' TSDoc comments by TypeDoc (`clients/web/typedoc.json`; both it
  and `book/` git-ignored). Never hand-edit those pages, and write the source's
  doc comments as TSDoc (`/** */`) — a Rust-style `///` is invisible to both
  the generator and the reader's editor.
- **Development** (section 3) — `docs/architecture.md`, `docs/contributing.md`,
  `docs/decisions.md` (the ADR-style design record, in the server book), the
  `PLAN.md` roadmaps (`PLAN.md` / `clients/python/PLAN.md` /
  `clients/gui/PLAN.md` / `clients/web/PLAN.md`), and the frozen
  `docs/history/build-log.md`. The manual-test surface is the runnable
  `examples/`, not a separate checklist.

Three rules hold across *all* of the above (repeat them to yourself):

1. **Plain Markdown only.** No Sphinx/RST directives in docstrings — use
   backticks and Google-style sections, never `:param:` field lists or `:role:`
   cross-refs. The reason is the *source* read, not the generators: a docstring
   is read first in the code, by whoever is editing the function, and RST
   markup is characters a human has to look past every time. That nothing in
   the chain speaks RST is a happy consequence, not the motive
   (`docs/decisions.md`, "Docstrings are Google-style Markdown").
2. **No milestone labels** (`Mx`/`Cx`/`Fx`) in any published doc or docstring.
   Those live only in the `PLAN.md` roadmaps.
3. **Prose names API actions by the API's own verbs.** When the protocol or API
   has a verb for an action, use *that* verb: a node or widget is **freed**
   (`/node_free`, `/gui_free`), never "destroyed"/"deleted"/"killed"; a def is
   **sent**/**loaded**; a server is **booted**; an element is **rendered**,
   never "realized". Verbs from *other* domains keep their own APIs' words
   (shell `kill`, `subprocess.terminate()`, POSIX "the kernel kills the
   process", upstream symbols cited verbatim).

The books cross-link each other by their ReadTheDocs URLs — all three are
published, so a new page links the other two rather than describing them. Each
carries a `.readthedocs.yaml` driving the build with `build.commands`, since RTD
has no native mdBook builder.

### Installing the two API-reference generators

Both go in **user space**, no sudo:

- **pydoc-markdown** (Python reference) — `uv tool install --python 3.12
  pydoc-markdown`. Pin 3.12: its deps lag the newest CPython, and 3.12 is also
  Read the Docs' version. Then `clients/python/docs/build.sh` regenerates
  `api.md` and rebuilds the book. (`uvx pydoc-markdown`, or `pip install` on a
  non-PEP-668 env, also work — see `clients/python/README.md`.)
- **TypeDoc** (TypeScript reference) — `npm install -g typedoc@0.28
  typedoc-plugin-markdown@4 typescript@5.9` (npm's prefix is under `~/.local`;
  symlink the `typedoc` bin into `~/.local/bin` like node's). It parses with
  **its own TypeScript 5.9** while the package compiles with the v7 in
  `node_modules`, and it runs with warnings as errors. The output file names in
  `clients/web/typedoc.json` are the contract with `src/SUMMARY.md`.

The web package is published to npm as `clausters` by the release tag —
`clients/web/BUILD.md`, "Publishing".

## 1. User documentation — Diataxis (Tutorial / Explanation / Reference)

Within user docs, think about **what the reader is doing**. Diataxis's quadrants
are an **internal lens** — a shared vocabulary so that when the user talks about
"user documentation and its sections" we mean the same thing. They are **not a
labeling scheme for the pages**: don't put "Tutorial" / "Explanation" /
"Reference" as headings or tags in a published page, and you needn't name the
quadrant out loud — just write the page in the right shape.

Clausters uses three of the four by default — **Tutorial, Explanation,
Reference**. The fourth, **How-to** (a goal-titled step recipe), is **not needed
by default and can be skipped**; write one only on **explicit request, for a
particular case**. The everyday practical-recipe niche is already covered by the
runnable `examples/` (catalogued in `examples.md`). One page is one kind, except
the feature pages (the documented hybrid below).

| Quadrant    | Reader is...         | Serves   | Voice                         |
| ----------- | -------------------- | -------- | ----------------------------- |
| Tutorial    | learning, new        | study    | "we will...; now run X"       |
| Explanation | understanding        | study    | "why it is so / how it works" |
| Reference   | building, wants fact | the work | dry, exhaustive, no opinion   |

**The two books have opposite balances — know which you are editing.** The
**Python book is the usage book**: it maps cleanly to user Diataxis — Tutorial
(`getting-started.md`), Explanation (`introduction.md`, `guide.md`), Reference
(`api.md`), plus the `examples.md` catalog; light and consumer-first. The
**server book leans heavily technical**, with only a thin usage layer: its
genuine usage docs are `getting-started.md` and `examples.md` (plus the
command-sending surface of `schemas.md`), while the deep `schemas.md` tables, the
feature pages, `ipc.md`, `using-as-a-library.md` and `clients.md` are
consumer-facing but **developer-grade** — they explain the system from the
inside. `architecture.md` and `contributing.md` are pure development (section 3).
Don't mistake a feature page's "how it works" for usage docs, and don't pad the
thin usage layer with internals — link to them instead.

- **Tutorial** — `getting-started.md` (one in each book), the single
  learning-path page: build / install, run, play a sound, render offline — start
  to a visible result, no alternatives, no theory.
- **Explanation** — the conceptual pages, **including every feature and
  subsystem page**: `introduction.md` (both books, orientation), `clients.md`
  (cross-language map), Python `guide.md` (the client layer by layer), the
  scheduling/grouping features `sample-clock.md`, `auto-order.md`, `parallel.md`,
  and the library / embedding subsystem pages `using-as-a-library.md` and
  `ipc.md`. These lead with the *why* and the *how-it-works*, compare to
  scsynth/supernova (or a network stack), and discuss trade-offs. Cross-link
  reference; don't restate it. `ipc.md` is the hybrid below: its lead and
  "Synchronous calls" are Explanation, while the transport table, the segment
  layout and the **embed C ABI** signatures are its embedded Reference block.
- **Reference** — the contracts and catalogs: `schemas.md` (defs / UGens / OSC
  commands — the wire format), `examples.md` (the examples catalog), the rustdoc,
  Python `api.md`. One uniform entry per component/example; complete and
  opinion-free.

**House style for feature pages (a deliberate hybrid).** Clausters keeps one
page per feature instead of splitting it across quadrants, so a feature page is
**Explanation-led with an embedded Reference block**, in this shape:

1. *Why* — the motivation and the scsynth/supernova contrast (Explanation).
2. *Protocol / Commands / Usage* — message signatures, flags and replies, as a
   tight table or code block (Reference).
3. *How it works* — the mechanism (Explanation).
4. *Caveats* — limits and gotchas (Explanation).

This is an intentional, accepted deviation from strict Diataxis. The *How it
works* part is developer-grade explanation living in a user page — that is much
of why the server book reads technical. When you write or extend a feature page,
keep that four-part shape and keep the Reference block factual and
self-contained: don't let the mechanism prose bleed into the command table, and
don't pass mechanism off as usage steps.

**Placing a new page:** pick the dominant quadrant, name the file, add it under
the right `SUMMARY.md` heading. The SUMMARY headings ("User Guide", "Library &
Embedding", "Developer Guide" in the server book; "User Guide", "Reference" in
the Python book) group by *audience*. A page is a single Diataxis kind — except
the feature pages, which are the documented hybrid above.

**Writing each kind:**

- *Tutorial* — imperative, concrete, deterministic. Every command runs exactly
  as written; end with something the reader can observe.
- *Explanation* — discursive; alternatives, rationale, comparisons allowed. No
  step-by-step.
- *Reference* — generated where possible; otherwise one uniform entry per
  command/UGen/function with arguments, types, defaults, replies and errors.
- *How-to* (only on explicit request, for a specific case) — title is the goal
  ("Render a score offline"): precondition -> numbered steps -> result, no
  background.

**`schemas.md` — the canonical wire-format reference.** It is the single source
of truth for the whole OSC surface (commands, the two def formats, UGen kinds)
and the only large reference with **no generator**, so keeping it in lockstep
with the server's actual command handling is a manual duty: any milestone that
adds or changes an OSC command, a def format or a UGen must update the tables
here — signatures, types, defaults, the `/done`/`/fail` replies and the exact
error strings. Two specifics:

- It is a **deep-link hub**: other pages anchor into its sections (e.g.
  `schemas.md#graphdef-...`, `#persisting-defs-across-restarts`,
  `#midi-standalone-bindings--boot-preset`). Don't rename a heading without
  fixing the inbound links.
- Like the feature pages it carries a few short inline notes (the feedback and
  disk-streaming paragraphs), but the per-command / per-UGen tables stay primary
  and opinion-free — push any longer rationale to the relevant feature page and
  link it.

**The embed/shm reference lives inside `ipc.md`.** That page is Explanation-led,
but it *contains* the canonical, hand-maintained reference for the local
surface: the **segment layout** (header / command plane / data plane) and the
**C ABI** function signatures. Both are **version-locked** — `clausters_abi_version()`
and the segment's layout version move in lockstep, and the doc must match the
ABI exported by `libclausters`. Treat it with the same sync duty as `schemas.md`:
a milestone that changes the C ABI or the segment layout updates the signatures,
the byte sizes/offsets and the version note here.

**`examples.md` is a catalog, not a component reference.** It is Reference-shaped
(a uniform table — one row per runnable demo: what it shows, how to run it) but
its job is **navigation**: it is the index of the practical, runnable layer,
routing the reader to the actual demos in `examples/`, `clients/python/examples/`
and the shell scripts, where the real "how do I do X" lives in code. Both books
have one (server `docs/examples.md` and the Python book's). Duties: keep it
**complete** — every demo has a row, and adding / renaming / removing one updates
the table in the same change; keep the deep-links live (rows link out to the
feature pages a demo exercises and to `schemas.md` anchors); and keep each row to
one factual line — the explanation belongs on the feature page, not here.

**`clients.md` is Explanation (the cross-language map), not Reference — and its
tables don't change that.** It carries two tables that *look* like reference but
are not authoritative. The **cdylib table** (`libclausters_ffi` / `libclausters`
/ `libclausters_midi` with a few key entry points) is a *map* — which library
plays which role; the authoritative contract is the rustdoc and the C ABI in
`ipc.md`, so keep it a pointer, not a second source of truth for signatures. The
**"Status at a glance"** table is project status (done/planned), the user-facing
snapshot of `PLAN.md` / `clients/python/PLAN.md` — keep it in sync with them and, like
all published docs, free of `Mx`/`Cx` labels. `clients.md` is also a **cross-book
hub**: it links the Python client book by its Read the Docs URL (an inline
comment flags the slug risk) — keep that link live.

**`using-as-a-library.md` is Explanation that defers to the rustdoc.** It is the
conceptual map for using the crate as a library, and it says so — the
authoritative API reference is the **rustdoc** (`cargo doc`), exactly as
`examples.md` routes to `examples/`. Two things to watch: its **Feature flags**
table is the reference-shaped part and it **overlaps `contributing.md`** (which
covers the same flags from the build / system-dependency angle) — keep the
feature *catalog* (what each flag adds) canonical here and the *build* angle in
`contributing.md`, cross-linked, not duplicated. And its code blocks are
illustrative `rust,ignore` snippets that **`mdbook test` does not compile**, so
they are hand-maintained against the real API — the rustdoc is the checked source
of truth.

## 2. API reference (autonomous spec, generated from source)

- **Rust** — document every public item with `///` / `//!`; the rustdoc is the
  reference. Keep examples in doc comments compiling (`mdbook test` /
  `cargo test --doc`).
- **Python** — write Google-style **Markdown** docstrings, then regenerate:
  `clients/python/docs/build.sh` runs pydoc-markdown into `src/api.md`. Edit the
  docstrings, never the generated `api.md`.

The plain-Markdown / no-RST rule from above is also what keeps these two
generators honest — a side benefit of a rule adopted for the source read.

## 3. Development documentation — internals & decisions (for core maintainers)

We follow the *spirit* of the standard formats, mapped onto existing files —
there is no separate `adr/` or C4 tree, by design:

- **Architecture (C4-style)** — `docs/architecture.md` is the developer guide:
  threads, memory lifecycle, invariants, how to add a UGen. Read it as the
  Container/Component zoom (the process, its threads and FIFOs, the modules);
  the Code level is the rustdoc. If you add a diagram, keep one zoom level per
  figure.
- **Contributing / governance** — `docs/contributing.md` is the CONTRIBUTING
  entry point: build/test, system deps, feature flags, RT-safety, the E2E
  sandbox rule, conventions. Keep every setup step runnable.
- **Decision records (ADR-style)** — the curated design record is
  `docs/decisions.md` (published in the server book). When a choice has
  non-obvious context and consequences, add a short entry there in ADR spirit:
  **context -> decision -> consequence**. The *what shipped* is the git history
  (there is no per-milestone log); the roadmap/rationale is the `PLAN.md` set;
  the frozen `docs/history/build-log.md` is not maintained/published as
  reference.

## 4. Examples

An example's *form* is its own subject — which of the three directories it
belongs to decides whether it is a notebook, a closed script or a page. That is
the **`examples` skill**; consult it before writing or editing one. What belongs
here: an example documents itself in its module docstring, and no book page
enumerates the examples (`examples.md` says only where the directories are and
how to run each family).

## Closing a milestone (documentation side)

When a feature is user-facing, "done" includes, where applicable:

- dev docs (`architecture.md`, module / rustdoc comments);
- the right Diataxis page(s) in `docs/` — and the Python book if it touches the
  client;
- a commented, explained entry in `examples/`;
- the `PLAN.md` roadmap checkbox updated;
- a `docs/decisions.md` entry only for a non-obvious choice.

Then rebuild both books (`mdbook build .` and `clients/python/docs/build.sh`) so
they still build clean.
