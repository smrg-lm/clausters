# Building verovio from `third_party/verovio`

How to build [verovio](https://www.verovio.org) — the engraving library behind
the notation track — from the source vendored in `third_party/verovio`, entirely
in user space (no sudo).

This mirrors `BUILD-FAUST.md` next to it: same pin-plus-script arrangement, same
prefix layout, same protections, same one shared library staged into the Python
wheel afterwards. What differs is *why* we build it at all.

## The vendored, reproducible build (`build-verovio.sh`)

The verovio source is **not committed** (it is heavy — a full clone is ~140 MB —
and git-ignored under `third_party/verovio`); reproducibility comes instead from
two committed files:

- **`third_party/verovio.pin`** — the single source of truth: the git remote
  (`VEROVIO_ORIGIN`, plain upstream — unlike the Faust pin, ours carries no
  patch of its own) and the exact commit (`VEROVIO_SHA`).
- **`third_party/build-verovio.sh`** — one recipe: fetch the pinned commit,
  build, install.

```sh
third_party/build-verovio.sh                    # libverovio into ~/.local
third_party/build-verovio.sh --prefix /some/where
```

It **protects an existing working clone** exactly as the Faust script does: if
`third_party/verovio` is checked out somewhere other than the pin it refuses to
move it — pass `VEROVIO_ALLOW_CHECKOUT=1` to check out the pin, or
`VEROVIO_SKIP_FETCH=1` to build the current checkout as-is.

## Why we build it at all: the released editor is dead

There is a published verovio, and for *drawing* it is fine. It is the **score
editor** that is not: in verovio 6.2.1, `Toolkit::SetViewAndEditor()` guards the
construction of the editor toolkit with

```cpp
#if defined NO_HUMDRUM_SUPPORT     // ... where it meant #ifndef NO_EDIT_SUPPORT
```

so in any build that *keeps* Humdrum support — the PyPI wheel included — the
guard never opens, `m_editorToolkit` stays null, and **every** `Toolkit::Edit()`
returns `false`. The parameterless `undo` returning `false` is what makes the
diagnosis certain (the parser would accept it unconditionally if the editor
existed), and the editor symbols *are* present in the shipped `.so`, so it is
not a `NO_EDIT_SUPPORT` build — it is the guard.

Upstream inverted it in [`8100cb396`][fix] ("Invert #define fix", 2026-05-27),
after 6.2.1. The pin is a `develop` commit past that fix — the source tree
versions itself 6.3.0-dev — which is why the pin is a commit and not a release
tag. **Repin to the 6.3.0 tag once it is released** — the pin moves, the recipe
does not: we build the library either way, because that is what gets bundled.

The rationale for choosing this route over the alternatives (waiting for 6.3, or
mutating the MEI in Python and re-engraving) is in
[`docs/decisions.md`](../docs/decisions.md).

[fix]: https://github.com/rism-digital/verovio/commit/8100cb39604d40102a9c2ce75719136f3fb52a77

## Requirements

System packages — `cmake`, `make` and a **C++20** compiler. That is the whole
list: verovio vendors its dependencies in-tree (pugixml, jsonxx, the MIDI and
Humdrum sources, libmei) and has **no submodules**, so unlike the Faust build
there is no LLVM, no zlib and nothing to pin a version of.

Nothing else: there is no language-specific build here, so no SWIG and no
Python development headers to hunt for.

## What the build carries (and what it leaves out)

verovio's importers are independent cmake options, so the build ships only the
input formats the client offers. **Kept:** MEI — the canonical one, and what the
edit cycle round-trips through — MusicXML and its compressed `.mxl` form, and
the two compact hand-typed formats, **ABC** and **Plaine & Easie**. **Dropped:**
Humdrum, GABC and DARMS.

Only Humdrum is a size argument, and a real one: it vendors humlib, ~148k lines,
and dropping it takes `libverovio.so` from **21 MB to 13 MB**. The
other two are noise — ABC and PAE measure about 10 KB apart — and are out
because nothing reads them, not to save anything.

Worth knowing if you ever change these flags: in 6.2.1 the editor and Humdrum
were *entangled* (the editor was guarded by Humdrum being off — the bug below),
so this same trim would have revived it. Past the pin they are independent, so
the build is small **and** editable with no coupling between the two.

The flags live in one list in the script (`vrv_options`).

## The artifact: `libverovio.so` + headers + resources

```sh
cmake -S third_party/verovio/cmake -B <build> \
      -DCMAKE_BUILD_TYPE=Release -DBUILD_AS_LIBRARY=ON \
      -DCMAKE_INSTALL_PREFIX=<prefix> \
      -DNO_HUMDRUM_SUPPORT=ON -DNO_GABC_SUPPORT=ON -DNO_DARMS_SUPPORT=ON
cmake --build <build> --parallel $(nproc)
cmake --install <build>
```

- `-DBUILD_AS_LIBRARY=ON` switches the cmake project from its default
  command-line tool to a shared `libverovio.so` (adding `tools/c_wrapper.cpp`)
  and turns on the install rules for the headers and the pkg-config file.
- `-DCMAKE_INSTALL_PREFIX` must be right at **configure** time, not just at
  install: verovio bakes its resource directory into the binary then
  (`add_compile_definitions(RESOURCE_DIR="${CMAKE_INSTALL_PREFIX}/share/verovio")`),
  and a toolkit that cannot find its SMuFL data engraves nothing. Getting this
  wrong produces a library that builds and links and then silently draws an
  empty page.

Installs `<prefix>/lib/libverovio.so`, `<prefix>/lib/pkgconfig/verovio.pc`, the
headers **flattened** into `<prefix>/include/verovio` (so the include is
`<verovio/toolkit.h>`, not a nested path), and the SMuFL/CSS resource data in
`<prefix>/share/verovio`. Like the Faust prefix, that makes it self-contained.

`BUILD_AS_LIBRARY` also adds `tools/c_wrapper.cpp`, a flat C API over the
toolkit (`vrvToolkit_loadData`, `_renderToSVG`, `_renderToTimemap`, `_getMEI`,
`_edit`, `_editInfo`, …). That is the surface **every** consumer uses: the
Python client binds it with `ctypes` (`clausters/gui/notation.py`), and a wasm
build would expose the same functions.

It is also what a future **native producer** would link — a C++ `DeviceContext`
emitting the display list directly instead of walking generated SVG.
`Toolkit::RenderToDeviceContext` is public and `DeviceContext` is an abstract
base of ~35 pure virtuals; see `docs/decisions.md` for the viability finding.

### Why there is no Python-module target

verovio also ships a SWIG Python module, built from the same sources through
scikit-build-core. We deliberately do **not** build it. It would be a second
compile of the same code, a second copy of the engine and its 12 MB of SMuFL
data in `site-packages`, and — worst — a distribution literally named `verovio`,
which pip can replace at any moment with the published one, whose editor is dead
(above). That is not hypothetical: it happened in this checkout, and the editing
tests started failing for an upstream reason.

The library has none of that ambiguity. It is ours, it is bundled where we put
it, and `clausters.gui.notation` loads it by path.

### The engraver in the Python wheel

`clients/python/build_native.py` copies `libverovio.so` and
`share/verovio` out of the prefix into `clausters/_libs/`, exactly as it already
does for libfaust, so an installed wheel engraves and edits with
nothing else on the machine and the client keeps `dependencies = []`.

The resource data has to travel with the library: verovio bakes its resource
path in at *configure* time (`RESOURCE_DIR`), pointing at the prefix it was
built for, and a staged copy is somewhere else entirely. `notation` finds the
data beside the library and passes it to each toolkit explicitly — a toolkit
that cannot find its SMuFL data engraves nothing.

Resolution order is the one every native artifact here follows:
`CLAUSTERS_VEROVIO` (a library file or a build prefix) → the bundled copy →
a system-wide install.

### Checking a build

The sanity check is the point of the whole exercise — perform a real edit and
confirm the toolkit accepted it. Through the C API, with no module installed:

```python
import ctypes, json, re
lib = ctypes.CDLL("libverovio.so")
lib.vrvToolkit_constructorResourcePath.restype = ctypes.c_void_p
lib.vrvToolkit_renderToSVG.restype = ctypes.c_char_p   # or the pointer truncates
tk = lib.vrvToolkit_constructorResourcePath(b"<prefix>/share/verovio")
lib.vrvToolkit_loadData(ctypes.c_void_p(tk), b"@clef:G-2\n@timesig:4/4\n@data:4CDEF/")
svg = lib.vrvToolkit_renderToSVG(ctypes.c_void_p(tk), 1, False).decode()
note = re.search(r'<g id="([^"]+)" class="note"', svg).group(1)
lib.vrvToolkit_edit(ctypes.c_void_p(tk), json.dumps(
    {"action": "drag", "param": {"elementId": note, "x": 0, "y": 20}}).encode())
# 0 with a null editor (the published build), 1 on this one
```

### `undo`/`redo` on an empty stack segfaults

Worth knowing before writing anything against the editor, and the reason the
check above drags a note rather than doing the obvious thing:

```python
# a loaded toolkit, then:
lib.vrvToolkit_edit(tk, b'{"action": "undo"}')  # SIGSEGV — and so does "redo"
```

On the *published* wheel that same call is the cleanest possible proof the
editor is dead: it returns `False` from the null-pointer branch without touching
anything. On a build where the editor actually exists it dereferences an empty
undo stack and takes the process down — so the probe that diagnoses the bug
crashes the fix. Confirmed here on `46a4df525`, exit 139, for both actions.

The consequence for our side: **never issue `undo`/`redo` unconditionally** —
gate them on `editInfo()`'s `canUndo` / `canRedo`. (Those two flags are also
looser than they look: a successful `drag` leaves `canUndo` `False`, yet a
following `undo` succeeds and sets `canRedo`. Treat them as a crash guard, not
as a model of the stack.)

## Licensing

verovio is **LGPL-3.0-only**; Clausters is **GPL-3.0-or-later**. LGPL-3 code
combines into a GPL-3 work without friction (that direction is the one the
licenses are written to allow), so linking it — dynamically as here — raises
nothing to resolve. Note it stays a *client-side* dependency: the server does
not link verovio, and neither does the GUI host (see below).

## Building clausters against it

Nothing in the workspace links verovio **today** — the notation track keeps it
strictly client-side, which is the whole reason the `score` widget consumes a
display list rather than a score. The GUI host draws the display list and knows
nothing about MEI; the web client reuses that same renderer against the
same seam. So:

- `cargo build` / `cargo test` need **none** of this. There is no feature flag
  to set and no `build.rs` probing for a prefix, so no Rust job in CI touches
  verovio.
- The Python client needs the **library**, built here and staged into its
  package by `build_native.py`. Nothing is installed into an interpreter.

CI builds it for exactly the two jobs that stage the package — the `python` job
and the release — through `.github/actions/verovio`, the composite that mirrors
the libfaust one (restore from cache, build from this pin on a miss).

A missing engraver fails the build on its own now: `build_native.py` requires
both vendored libraries by default and stops with the recipe. Both jobs also set
`CLAUSTERS_REQUIRE_COMPLETE=1`, which refuses a deliberate `CLAUSTERS_SKIP_*`,
so nothing can opt out of the engraver (or of a def family) on the way to a
release. Without that, a green run and a wheel whose `score` widget raises at
the user's run time look identical: the notation tests skip themselves when
there is no engraver.

When the native producer lands, it will link this prefix from its own crate
(**not** the GUI host — that would break the seam), and only then does a verovio
prefix become a build dependency of anything in the workspace.

## Verified 2026-07-24 (this build)

verovio 6.3.0-dev (`46a4df525`, `develop` of 2026-07-10) on Ubuntu, cmake 4.2.3
+ g++ 15.2.0. No submodules to fetch; no system package had to be installed.

- **The library** into a temporary prefix: 298 objects, **4 min 32 s** on 12
  cores. Produces `lib/libverovio.so` (21 MB before the importer trim, 13 MB
  after, 11 MB stripped as staged), 311 headers flattened into `include/verovio`
  (3.8 MB), `share/verovio` (12 MB) and a `verovio.pc` reporting **6.3.0**.
  Confirmed the resource path is baked into the `.so` as
  `<prefix>/share/verovio`.
- **Bound with `ctypes` and staged into the package**, the engraver works with
  no `verovio` module installed in the interpreter at all: the client's suite
  runs **373 passed, 3 skipped** resolving the library out of `_libs/`.
- **The editor works.** `drag` on a note returns `True` and a following `undo`
  returns `True` with `canRedo` set. On the 6.2.1 wheel the same session returns
  `False` for every `edit()`, `undo` included.
- Also found: `undo`/`redo` on an empty stack segfault (see above).
