# clausters-editor

A Python editor with an interactive session: the code, a *post* window with
the output and a Markdown documentation viewer, drawn by the clausters GUI
host in one window (its dock), which also hosts the project's applications
(the multitrack, the score, notes and audio editors), each on a page of its
own. It works with any Python, clausters or not.

It is a crate of the repository's workspace: a library, the application
(`src/app.rs`), with what it asks of the program it runs in behind a
`Platform` trait (`src/platform.rs`), and the binary, which links the host
and runs the application as a client of it in its process (`src/native/`).
The same application built for a browser is the editor as a page, with a
JavaScript session (`page/`, below).

## Building and running

From the repository's root, with Rust (the pinned toolchain) and, for the
score editor, libverovio built as `third_party/BUILD-VEROVIO.md` says (the
default `score` feature links it; `--no-default-features` leaves it out):

```sh
cargo run -p clausters-editor --release -- script.py other.py   # each file in a tab
cargo run -p clausters-editor --release -- --help
```

`python/driver.py` is the session's driver, compiled into the binary, and
`python/test_driver.py` speaks to it as a subprocess:

```sh
python3 -m unittest discover -s crates/clausters-editor/python -p 'test_*.py'
```

## The editor as a page

`page/` is the editor in a browser tab: the GUI host's dock across the tab,
the application built for wasm32 (`src/page.rs`), a JavaScript session in a
worker on the clausters web client's engine and the TypeScript service in
another. Its build builds the web client too (`clients/web`, whose GUI host
draws the applications), so the page never serves applications older than
the editor; with `CLAUSTERS_WEB` naming a package of its own, that one is
served as it is:

```sh
cd crates/clausters-editor/page
npm ci
./build.sh        # the web client, the application for wasm32, the page
npm run serve     # serves it, with the clausters web package at /clausters/
npm test          # drives it in headless Chrome (-- firefox: in Firefox)
```

The page is served cross-origin isolated (`Cross-Origin-Opener-Policy:
same-origin`, `Cross-Origin-Embedder-Policy: require-corp`): only then does it
load the GUI host with the applications, so a server of its own sends the two
headers too. `docs/page.md` is its user documentation.

## Installing for the user

```sh
./install.sh install            # builds and installs into ~/.local (binary, docs, licenses, menu entry)
./install.sh update             # rebuilds and replaces (works with the app open)
./install.sh uninstall          # removes the installation; keeps configuration and environments
./install.sh uninstall --purge  # also deletes configuration, virtual environments and cache
```

Options: `--prefix DIR` (another destination), `--no-build` (use the binary
already built), `-y` (no confirmation). See `./install.sh --help`. The book
goes to `share/clausters-editor/docs`, where the binary finds it.

## The Python package

`wheel/` packages the application as a Python wheel, `clausters-editor`, which
the clausters client installs as its `editor` extra (`pip install
clausters[editor]`). Its `clausters-editor` command runs the bundled binary with
the interpreter the package is installed in as the session's default, so the
session imports what that environment has until another one is chosen.

```sh
pip install build                                        # once: the PEP 517 front end
python -m build --wheel crates/clausters-editor/wheel    # builds the binary and the wheel
CLAUSTERS_EDITOR_SKIP_BUILD=1 python -m build --wheel crates/clausters-editor/wheel
```

From the repository root, `clausters[editor]` installs with both paths in one
command; pip takes the second as the `clausters-editor` the extra names, as it
would take PyPI's:

```sh
pip install -e "./clients/python[editor]" ./crates/clausters-editor/wheel
```

The wheel carries the binary, its book, the third-party notices and, on
Linux, libverovio with its data (`_libs/`, which the binary's run path names);
its version is the workspace's. The Linux tag is the build machine's glibc
(`manylinux_X_Y`), so the published wheel is built on the oldest Ubuntu CI
offers.

### A test installation

To try a version without touching the real installation or its data: install it
into a temporary folder and redirect the XDG folders where the app keeps its
configuration, virtual environments and cache.

```sh
T=$(mktemp -d)
export XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache

./install.sh install --prefix "$T/pfx"
"$T/pfx/bin/clausters-editor"             # the app starts with no previous configuration or environments

./install.sh uninstall --purge -y --prefix "$T/pfx"   # deletes only what is in $T
rm -rf "$T"
```

The XDG variables only affect the terminal they are exported in. The menu
entry does not show up in the applications menu, because `$T/pfx/share` is not
among the folders the desktop looks in.

## Environments and configuration

- **Virtual environments** (Session → Python Environments): created in
  `~/.local/share/clausters-editor/venvs/<name>` with the default
  Python's `-m venv` (see `CLAUSTERS_EDITOR_DEFAULT_PYTHON` below), and
  the *install source* (the arguments of `pip install`, e.g. `/path/to/package`
  or `-e /path/to/package`) is installed into them. The session uses the last
  environment chosen, also after reopening the app.
- **Documentation folder** (File → Preferences, or Help → Documentation
  Folder): any folder with `.md` files; by default, the one bundled
  with the app.
- **Package books**: beside that folder the viewer shows the book of every
  package of the session's environment that registers one. A package registers
  a book with an entry point in the `clausters_editor.docs` group, named after
  the book: a callable that returns a folder of `.md` files. The clausters
  client registers two this way, its own and the server's. Search and help reach them too; they are asked for
  again each time the session starts.
- **Books and the start page**: a folder that carries mdBook's table of
  contents, `SUMMARY.md`, is read as the book it describes. Its first heading
  is the book's title (unless it is mdBook's own `# Summary`), each later
  heading a part and each `[Title](page.md)` a chapter, nested by indentation.
  A folder without one is a book listing its pages by their first heading. The
  start page (`F1`, the house button) shows every book with its parts and
  first-level chapters, a book's title opens its whole table of contents, and
  the bar says which book and chapter is on screen.
- **Interrupt hooks**: `Ctrl+.` interrupts the session's code, then calls the
  function every imported package registers in the `clausters_editor.interrupt`
  group, so the key also stops what a package is still doing on its own (the
  clausters client stops every session's sound this way). A package that is not
  imported is never imported by it.
- **Startup code** (File → Preferences): runs at the start of every session.
- **Indentation** (File → Preferences): what `Tab` inserts -- a tab character,
  2 spaces or 4 (the default).
- **Reopening the last session's files** (File → Preferences, off by default):
  at startup each editor opens again the files it had open, and shows the one
  that was on screen.
- The environment, the documentation folder and the startup code are kept in
  `~/.config/clausters-editor/config.json`; the theme, the size of the text,
  the indentation, the panels' arrangement, the open files, File → Open
  Recent's list and the window's size and place in `settings.json` beside it.
  The folders are the platform's own under the program's name
  (`$XDG_CONFIG_HOME`, `$XDG_DATA_HOME` and `$XDG_CACHE_HOME` on Linux). The
  first start moves the webview editor's folders
  (`io.github.smrg-lm.clausters.editor`) there, and builds its environments
  again where they moved, with their progress in the post window.
- `CLAUSTERS_EDITOR_PYTHON=/path/python` forces an interpreter (it takes precedence over
  the environment).
- **Completions, signatures and hover help** come from
  [Jedi](https://jedi.readthedocs.io/) running in the session (`python/driver.py`),
  which reads the file and the session's objects together. The Python package
  depends on it and every environment the app creates installs it; elsewhere the
  session still runs code, and says once that `pip install jedi` adds them.
- `CLAUSTERS_EDITOR_DEFAULT_PYTHON=/path/python` is the interpreter used when no
  environment is active, and the one new environments are created from (`python3`
  when unset). The Python package's command sets it.

## License

**GPL-3.0-or-later**: see the repository's `COPYING`.

### Third-party licenses

The editor ships the license of every third-party work it is built with, in
`licenses/THIRD-PARTY.txt`, because most of those licenses ask for it. The file
is generated, never edited, and it has to follow the dependencies: **adding,
removing or updating a library the editor ships -- a crate the binary links,
an npm package of the page -- ends with `node scripts/notices.mjs` and the
regenerated file in the same commit.** CI runs it with `--check` and fails
when the file is not current.

- **Before adding a library, check its license against GPL-3.0-or-later.** A
  crate under a license `about.toml` does not accept stops `cargo about` with
  an error: add the license to `accepted` only after that check, never to get
  past the error.
- What counts as shipped: the page's npm dependencies (`page/package.json`),
  the faces the GUI host embeds, libverovio, and the crates `cargo about`
  resolves for every release platform, less the project's own.

It needs the page's dependencies installed (`npm ci` in `page/`) and `cargo
about` (`cargo install --locked cargo-about --features cli`; CI pins 0.9.2).
