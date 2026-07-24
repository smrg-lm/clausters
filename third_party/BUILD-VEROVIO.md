# Building verovio from `third_party/verovio`

How to build [verovio](https://www.verovio.org) — the engraving library behind
the notation track — from the source vendored in `third_party/verovio`, entirely
in user space (no sudo).

This mirrors `BUILD-FAUST.md` next to it: same pin-plus-script arrangement, same
prefix layout, same protections. What differs is *why* we build it at all, and
that there are two artifacts rather than one.

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
third_party/build-verovio.sh --python           # the Python module instead
third_party/build-verovio.sh --library --python # both
```

It **protects an existing working clone** exactly as the Faust script does: if
`third_party/verovio` is checked out somewhere other than the pin it refuses to
move it — pass `VEROVIO_ALLOW_CHECKOUT=1` to check out the pin, or
`VEROVIO_SKIP_FETCH=1` to build the current checkout as-is.

## Why we build it at all: the released editor is dead

The Python client engraves through the **published PyPI wheel**, and for
*drawing* that wheel is fine. It is the **score editor** that is not: in verovio
6.2.1, `Toolkit::SetViewAndEditor()` guards the construction of the editor
toolkit with

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
tag. **Repin to the 6.3.0 tag once it is released**: that is the point where the
published wheel becomes usable again and this build stops being mandatory for
the Python client. It stays required for the native producer.

The rationale for choosing this route over the alternatives (waiting for 6.3, or
mutating the MEI in Python and re-engraving) is in
[`docs/decisions.md`](../docs/decisions.md).

[fix]: https://github.com/rism-digital/verovio/commit/8100cb39604d40102a9c2ce75719136f3fb52a77

## Requirements

System packages — `cmake`, `make` and a **C++20** compiler. That is the whole
list: verovio vendors its dependencies in-tree (pugixml, jsonxx, the MIDI and
Humdrum sources, libmei) and has **no submodules**, so unlike the Faust build
there is no LLVM, no zlib and nothing to pin a version of.

The `--python` target also needs **SWIG** and the **Python development headers**.
SWIG does not have to be installed — `pyproject.toml` lists it in
`build-system.requires`, so pip pulls it from PyPI into the isolated build
environment. The headers are the one thing a distro splits into a `-dev` package
you would need sudo for, and the script routes around that; see below.

## What the build carries (and what it leaves out)

verovio's importers are independent cmake options, so the build ships only the
input formats the client offers. **Kept:** MEI — the canonical one, and what the
edit cycle round-trips through — MusicXML and its compressed `.mxl` form, and
the two compact hand-typed formats, **ABC** and **Plaine & Easie**. **Dropped:**
Humdrum, GABC and DARMS.

Only Humdrum is a size argument, and a real one: it vendors humlib, ~148k lines,
and dropping it takes the built Python wheel from **8.2 MB to 5.2 MB**. The
other two are noise — ABC and PAE measure about 10 KB apart — and are out
because nothing reads them, not to save anything.

Worth knowing if you ever change these flags: in 6.2.1 the editor and Humdrum
were *entangled* (the editor was guarded by Humdrum being off — the bug below),
so this same trim would have revived it. Past the pin they are independent, so
the build is small **and** editable with no coupling between the two.

Both targets take the flags from one list in the script (`vrv_options`), the
`--python` one via scikit-build-core's `SKBUILD_CMAKE_DEFINE`, so the library
and the Python module can never drift apart in what they can read.

## The two targets

They are separate compiles of the same sources, so asking for both costs two
builds.

### `--library` (the default) — `libverovio.so` + headers + resources

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

This is the artifact a **native producer** links against — the C++
`DeviceContext` implementation that would emit the display list directly instead
of walking generated SVG. `Toolkit::RenderToDeviceContext` is public and
`DeviceContext` is an abstract base of ~35 pure virtuals; see
`docs/decisions.md` for the viability finding.

### `--python` — the `verovio` module

pip drives the *same* cmake project through scikit-build-core (the root
`pyproject.toml` sets `BUILD_AS_PYTHON`), which builds a static library plus the
SWIG extension module. It installs into whatever interpreter `$PYTHON` is —
**activate the virtualenv first**, or point `PYTHON` at it — and
`--force-reinstall` is what makes it replace an already-installed PyPI wheel of
the same version.

The script builds and installs in **two steps** rather than one
`pip install <src>`, because the two can need different interpreters:

```sh
<builder> -m pip wheel --no-deps --wheel-dir <dir> third_party/verovio
<target>  -m pip install --force-reinstall <dir>/verovio-*.whl
```

Compiling needs the **development headers**, which a distro Python often ships
in a separate `-dev` package. But the extension is built against the **stable
ABI** (`Py_LIMITED_API` 3.10, so the wheel is tagged `cp310-abi3`), which means
the wheel *any* CPython ≥ 3.10 builds is installable into *any* other. So when
the interpreter you actually use has no headers, build with one that does:

```sh
uv python install 3.12
PYTHON=.venv/bin/python VEROVIO_BUILD_PYTHON="$(uv python find 3.12)" \
  third_party/build-verovio.sh --python
```

That is not a corner case here: this repo's root `.venv` runs on Ubuntu's
`/usr/bin/python3.14`, which has no headers installed, so the plain form fails
in cmake's `FindPython`. The script checks for `Python.h` up front and prints
exactly the two commands above instead of letting the build fail 200 lines deep.

The script's sanity check is the point of the whole exercise — it performs a
real edit and confirms the toolkit accepted it:

```python
tk.loadData("@clef:G-2\n@timesig:4/4\n@data:4CDEF/")
note = re.search(r'<g id="([^"]+)" class="note"', tk.renderToSVG(1)).group(1)
tk.edit({"action": "drag", "param": {"elementId": note, "x": 0, "y": 20}})
# False on the published wheel (null editor), True on this build
```

### `undo`/`redo` on an empty stack segfaults

Worth knowing before writing anything against the editor, and the reason the
check above drags a note rather than doing the obvious thing:

```python
tk = verovio.toolkit(); tk.loadData(...)
tk.edit({"action": "undo"})     # SIGSEGV — reproducible, and so does "redo"
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
nothing about MEI; a future JS/wasm client reuses that same renderer against the
same seam. So:

- `cargo build` / `cargo test` need **none** of this. There is no feature flag
  to set, no `build.rs` probing for a prefix, and CI does not build verovio.
- The Python client needs the module — from PyPI for drawing, from
  `--python` here for editing.

When the native producer lands, it will link the `--library` prefix from its own
crate (**not** the GUI host — that would break the seam), and only then does a
verovio prefix become a build dependency of anything in the workspace.

## Verified 2026-07-23 (this build)

verovio 6.3.0-dev (`46a4df525`, `develop` of 2026-07-10) on Ubuntu, cmake 4.2.3
+ g++ 15.2.0. No submodules to fetch; no system package had to be installed for
either target.

- **`--library`** into a temporary prefix: 298 objects, **4 min 32 s** on 12
  cores. Produces `lib/libverovio.so` (21 MB), 311 headers flattened into
  `include/verovio` (3.8 MB), `share/verovio` (12 MB) and a `verovio.pc`
  reporting **6.3.0**. Confirmed the resource path is baked into the `.so` as
  `<prefix>/share/verovio`.
- **`--python`**, built by a uv-managed CPython 3.12 and installed into the
  root `.venv` (Ubuntu's Python 3.14, no headers): produces
  `verovio-6.3.0.dev102-cp310-abi3-linux_x86_64.whl` (8.6 MB), which replaces
  the PyPI `verovio` 6.2.1 and imports cleanly on 3.14 — the stable ABI
  carrying across four minor versions as intended.
- **The editor works.** `drag` on a note returns `True` and a following `undo`
  returns `True` with `canRedo` set. On the 6.2.1 wheel the same session returns
  `False` for every `edit()`, `undo` included.
- The Python client's suite is unaffected by the version move: **364 passed, 3
  skipped**, same as on 6.2.1.
- Also found: `undo`/`redo` on an empty stack segfault (see above).
