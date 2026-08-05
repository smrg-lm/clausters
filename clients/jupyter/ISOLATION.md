# Isolating the notebook track

An audit of what the `clausters-jupyter` work actually put into the rest of the
repository, and a plan for moving it onto a branch of its own.

The reason for the move is not that the package is unwanted. It is that a few
of the seams it opened in the two clients came out wrong, and reworking them
under a running notebook is the slow way to do it. What follows separates three
things that the commit history mixes: what is the notebook's alone, what is
general and merely arrived with it, and the small set of hooks that are general
in *shape* while having exactly one consumer.

Audited range: `3dd4882..d6e9b5a` (2026-08-03 / 2026-08-04, 39 commits), plus
the two immediate ancestors `f3f0e9c` and `92a7fdd`, which are notebook
preparation under other names.

---

## 1. The server is clean

`src/`, `crates/` and `tests/` contain no notebook code at all — a grep for
`jupyter|notebook|anywidget|ipykernel` returns nothing. The seven commits in the
range that touch the server belong to other tracks (ring frames carrying an
author, bulk samples as a blob, client-side buffer writes, the embed buffer
door, `OscFunc` over an existing connection, offline render, the clock grid).

The only server-side surface is documentation: the section at
`docs/architecture.md:559` and one clause in the Clients paragraph at
`docs/architecture.md:167`.

**Nothing to isolate here.** The branch does not need to touch the Rust
workspace.

---

## 2. Notebook-only: moves as it stands

Nothing outside the notebook imports any of this.

| What | Lines |
| --- | --- |
| `clients/jupyter/` entire (8 modules, 4 tests, 3 example notebooks) | ~3400 |
| `clients/web/src/notebook/{widget,client}.ts` | 1148 |
| `clients/web/tests/notebook.{html,test.ts}` | 550 |
| `clients/python/docs/src/notebook.md` + its `SUMMARY.md:32` entry | 222 |
| `docs/architecture.md` §"The notebook package: where it lives" (559–626) | ~60 |
| `docs/decisions.md` entries at 4166, 4518, 4566 | — |
| `scripts/refresh-web.sh` | 47 |
| `.gitignore` block (staged `_web/`, `static/`, scratch notebooks, paired `.ipynb`) | 14 |
| `pytest.ini` (`pythonpath`/`testpaths` second entry) | 2 |
| `clients/web/build.sh` esbuild step for `dist/notebook-client.js` (89–106) | ~18 |
| `clients/web/tools/check-package.mjs:57-58` (two shipped paths) | 2 |
| `clients/web/test.sh:171` (`run_page notebook.html`) | 1 |
| Roadmap entries: `clients/python/PLAN.md` §193 (C38, C39), `clients/web/PLAN.md` W19 (1133) and the esbuild bullet (80) | — |

CI mentions the notebook nowhere, so no workflow changes are involved.

**The one that is not merely a file.** `clients/web/src/notebook/` is not a
directory sitting beside the web client — it is built by a dedicated esbuild
step, asserted into the npm tarball by `check-package.mjs`, exercised by its own
page in `test.sh`, and fronted by `client.ts`, a re-export entry whose own
comment states the constraint: *adding a name there is how the front end grows;
adding one anywhere else is how it stops loading*. Every npm consumer of the web
client currently ships the notebook front end. That coupling is the single
largest thing this branch buys back.

---

## 3. The hooks: general in shape, one consumer each

These live in the reference client (and its TypeScript port) and nothing but the
notebook uses them. They are where the design needs work.

| Hook | Where it lives | Sole consumer |
| --- | --- | --- |
| `interface.boot()` — undeclared duck-typed hook | `defs/server/__init__.py:214` | `carrier.py:121` |
| `OscInterface.awaitable` + the branch in `send_def` | `base/_oscinterface.py:155`, `defs/_wire.py:41` | `carrier.py:103` |
| `GuiHost.local_files` + the blob branch in `plot()` | `gui/host.py:67`, `plot.py:201-214` | `session.py:188` |
| `Session.adopt_gui` / `Session.adoptGui(host, {page})` | `session.py:316`, `session.ts:292` | `session.py:195` |
| `scope()` waiving its shared-memory requirement for a registered host | `scope.py:189` | the notebook's host |
| `setTickWorkerUrl` | `base/clock.ts:151` | `widget.ts:482` |

### What is actually wrong

**`interface.boot()` is the worst of them, and should be redesigned rather than
ported.** It is reached by `getattr(self.interface, "boot", None)` in the middle
of `Server.boot`, and it is declared on no base class — `OscInterface` does not
mention it, so there is no place a reader can learn the protocol exists. Worse
is what the single implementer does with it: `CommInterface.boot` starts
nothing. It checks `link.showing()` and, finding no cell, emits a warning. A
caller asking to boot a server gets back an advisory. The inversion is on both
ends, and the honest version of this is probably a capability the carrier
declares (like `awaitable` and `stream`) plus an explicit verb, not a hook that
happens to be named `boot`.

**`adoptGui(host, {page})` carries a live asymmetry.** The `page` option exists
in TypeScript and has no Python counterpart, for a real structural reason — in
TS a `GuiHost` is only the client half, while the wasm instance (GPU device and
drain loop) is a separate object that would be orphaned. `clients/web/PLAN.md`
(1580–1590) already records the surrounding tangle, including `connectGui`,
which that file proposes dropping. The two should be settled together.

**`awaitable` and `local_files` default to `True`**, so removing only their
consumers leaves dead branches rather than breakage. That makes them the
cheapest to defer.

---

## 4. General, and must not move

Some of this arrived in notebook commits, which makes it easy to mistake for
notebook code.

- **`clients/python/clausters/base/bulk.py`** (`samples_to_blob`,
  `blob_to_samples`) is general buffer machinery from `eb97adf`, consumed by
  `defs/buffer.py:332` (`Buffer.set_samples`), `gui/guidef.py` and the web
  client. Only the branch *selection* in `plot.py` is entangled: the blob path
  is right on its own merits (a TCP host on another machine cannot map a temp
  path either), and what is notebook-specific is the flag that chooses it. The
  clean cut keeps the branch and removes the flag.
- **`IdShare` / `share_of` and the `share=` threading** (`base/ids.py`,
  `base/core.ts`, and the allocators in node, bus, buffer, server and gui ids in
  both clients) is a **property of the server's id model, not of notebooks** —
  it belongs with the protocol, and `docs/schemas.md:57` documents it there.
  The server partitions node ids into one client range and every client
  allocates from it; nothing on the wire arbitrates between two of them, so
  clients that share a server split it client-side by equal slices in a fixed
  order. That is the same problem scsynth answers with a client id, and it
  arises for **any** two clients: two Python processes on one server, a Python
  client beside a web page, a sequencer beside a separate GUI process. The
  mechanism itself is forty lines of arithmetic with no notion of a kernel, a
  comm or a page, and `share=` / `share?: IdShare` is a public constructor
  argument on both clients.

  What is the notebook's is only the **call sites** — `KERNEL_SHARE`
  (`clausters_jupyter/session.py:182,192`) and `PAGE_SHARE`
  (`notebook/widget.ts`) — which leave with section 2. It is currently the only
  in-tree configuration that instantiates a split, which is what made it look
  notebook-specific; it is not.
- **`newGuiHost` / `newPools`** (`gui/page.ts`, `base/pool.ts`) were introduced
  by the first notebook commit but were adopted afterwards by `63940fa` (the web
  client's `Session`); both are exported from `index.ts` and `runtime.ts` today.
  They are general now.
- **`Server.boot` and `GuiHost.boot` as instance methods** (was: classmethods).
  General, an improvement on its own, and load-bearing for everything since.
- **`set_ambient_host` / `ambient_host`** predate this work (`43edc80`,
  2026-08-02). Only the waiver in `scope.py:189` is the notebook's.
- Everything in `eb97adf`, `531836e`, `80500b3`, `c423ffe`, `85bc723` — the
  buffer and shared-memory track.

---

## 5. Tests, mixed

`clients/python/tests/test_id_share.py` (94) and its counterpart
`clients/web/tests/share.test.ts` (128) **stay on `main`**: they assert the
slicing arithmetic, which is general (see section 4). Only their opening
docstrings name a notebook as the motivating case, and that is prose to reword.

Individual cases inside shared files: `test_defs.py:262` (a Jupyter comm queuing
its own reply), `test_plot.py` (the blob path), `test_scope.py` (the waived
segment), `test_session.py` (two adoption cases), `session.test.ts` (adoption),
`hosts.html` (two hosts in one page — arguably general, since the capability is
the web client's).

---

## A plan

Ordered so that each step leaves `main` green on its own.

1. **Branch and move section 2 verbatim.** Files only, no edits to shared code.
   `main` loses the package, the front end, its tests, its docs pages and its
   build/test/package wiring. Verify with `./build.sh && ./test.sh` from
   `clients/web` (it must pass with `notebook.html` and the esbuild step gone),
   `pytest` from the root, and a docs build for the two book pages that lose an
   entry.
2. **Revert the hooks one commit per hook**, in this order — cheapest and least
   entangled first, so the hard one is faced alone:
   1. `setTickWorkerUrl`
   2. `scope()`'s waiver
   3. `awaitable` (+ the `send_def` branch, + `test_defs.py:262`)
   4. `local_files` (+ the `plot.py` branch selection, + `test_plot.py`) —
      **keeping `base/bulk.py` and the blob path itself**
   5. `adopt_gui` / `adoptGui` (+ their tests), together with a decision on
      `connectGui` per `clients/web/PLAN.md`
   6. `interface.boot()` — remove the `getattr` from `Server.boot`
3. **Reword the prose that names the notebook as the motivating case** for
   something that stays: `docs/schemas.md:57`, `base/ids.py:16`,
   `test_id_share.py:5`, `docs/architecture.md:167`. Each should state the
   general configuration (two clients on one server) and drop the example that
   is no longer in the tree.
4. **Leave a note where the roadmaps lose their entries.** C38/C39 and W19
   should say the track moved and where, rather than disappearing — otherwise
   the next reader re-derives the whole design from the git log.
5. **On the branch, rework rather than re-apply.** The two that should be
   redesigned before they come back are `interface.boot()` (a carrier that
   cannot start its peer needs to say so, not to warn from inside a boot) and
   the `adoptGui` `page` asymmetry. Note that the branch will be the only
   consumer of `IdShare` and of the blob bulk path, both of which stay on
   `main`: it consumes them, it does not carry them.

### Not verified

The claim that step 1 breaks nothing comes from reading importers, not from
running the suites with the files removed. Steps 2.3 and 2.4 are asserted to be
dead-branch removals on the basis that both flags default to `True`; neither was
executed with the flag gone.
