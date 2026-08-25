# Examples

The examples live in the **repository**, not in the wheel: `pip install
clausters` gives you the client, a checkout gives you the scripts. There are
three directories of them —

- `clients/python/examples/` — the installed-package scripts: they import
  `clausters` the way your own code does (no `sys.path` shim, no `target/`
  directory), including the `panels/`, `views/` and `editors/` folders that
  drive the GUI host;
- `examples/` at the repository root — the lower-level demos: the transports,
  the raw OSC helpers, the audible tours of the UGen families. Those use a
  `sys.path` shim, so they run from a checkout with no install.

Run them after installing the package (see [Getting started](getting-started.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
python clients/python/examples/basics/hello_note.py
```

They are organized one folder per subject — `basics/`, `spectral/`, `buffers/`,
`transport/`, `io/`, `faust/`, and the three that need a display and a GPU
adapter: `panels/` (controls and layout), `views/` (reading something) and
`editors/` (writing something). The directory's own README says what each folder
holds.

**Each example documents itself.** Its module docstring says what it shows, what
it needs — nothing at all (most render offline, no server and no audio device),
a running server, or a display and a GPU adapter — and how to run it. Start with
`basics/hello_note.py` (the shortest path to sound) and `basics/verbs.py` (every
playable kind through one verb); after that, the directory listing plus each
file's first paragraph is the catalog.

Many examples have a **web counterpart in the same folder under the same name**
in `clients/web/examples/` — the same instrument written as a page instead of a
script, sounding through the browser's in-page engine. The two directories carry
the same layout on purpose: one example in two languages is easiest to read when
the pair is easy to find.

The GUI examples drive the **clausters GUI host**, a separate process (or a
browser tab) this client talks to over the widget protocol. The host itself,
including the browser quick-start, is documented in the [Clausters server
book](https://clausters.readthedocs.io/)'s clients chapter. Two of them close
the composition loop: `multitrack.py` lays out tracks of clips on one shared
time axis, and `composer.py` drives that view from a composition — the
arrangement drawn, dragged and re-rendered (see [Composition: the arrangement
and the multitrack editor](composition.md), and the step-by-step tutorial
[Composing a piece](composing.md), which builds the same piece interactively).
