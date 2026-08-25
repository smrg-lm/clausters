# Clausters — GUI client track

[![Server book](https://readthedocs.org/projects/clausters/badge/?version=latest)](https://clausters.readthedocs.io/)
[![Python client book](https://readthedocs.org/projects/clausters-python/badge/?version=latest)](https://clausters-python.readthedocs.io/)
[![Web client book](https://readthedocs.org/projects/clausters-web/badge/?version=latest)](https://clausters-web.readthedocs.io/)

📖 **Documentation:** [The GUI protocol](../../docs/gui-protocol.md) in the
[server book](https://clausters.readthedocs.io/) (the `/gui_*` wire reference)
· [Python client book](https://clausters-python.readthedocs.io/) (driving it
from Python) · [web client book](https://clausters-web.readthedocs.io/)
(driving the host in a browser, on a canvas)

The scriptable GUI host for the [Clausters](../../README.md) audio server: a
language client sends a **GuiDef** — a widget tree as JSON riding OSC — over
the `/gui_*` vocabulary, and the host renders it, feeds it live server data
(shared-memory control buses, audio taps, buffers), and sends value changes
and edits back. A platform-agnostic core (widget model, layout, signal logic,
flat-geometry drawing) sits behind small traits, with two fronts: the
**native** host (winit + wgpu) and the **browser/wasm** host on a canvas.

This crate is an **independent workspace** — deliberately not a member of the
root `clausters` workspace, so the GUI track never entangles the core server
build.

## Quick start

```sh
cd clients/gui
cargo build --release
./target/release/clausters-gui          # the host; UDP+TCP on port 57210
./target/release/clausters-gui --help   # ports, --server, --headless, --standalone…
```

Then drive it from a client over OSC — see
`clients/python/examples/views/window.py` (windowed) and `skeleton.py`
(protocol only), or the `clausters.gui` Python submodule, which builds GuiDefs
and launches the host for you (`Session.gui()`).

> Testing through the Python launcher? The editable package's **bundled**
> binary in `clients/python/clausters/_bin/` wins over this workspace build —
> refresh it (or set `CLAUSTERS_GUI_BIN`) after rebuilding. See the repo
> `CLAUDE.md` / `clients/python/README.md`.

**Drawing text with a real typeface** is an opt-in build: the crate's own
`font-atlas` feature adds a glyph rasterizer and one atlas texture per window,
where the default build draws its embedded 5x7 bitmap face. It ships no face —
point it at one, or let it find one of the system's:

```sh
cargo build --release --features font-atlas
./target/release/clausters-gui --font /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf
```

Nothing else changes: the sizing table never followed the typeface, so the same
GuiDef lays out identically either way, and a build with the feature but no face
draws exactly what the default build draws.

**What a build pays for** is the rest of the feature set. Two element families
are optional and **on by default** — `notation` (the `score` element and the
`lyon` tessellation behind it) and `patcher` (the `plane`-with-boxes graph
view) — so a program that links this crate for its controls can drop what it
never draws:

```sh
cargo build --release --no-default-features --features midi
```

A family compiled out drops its rows from the element table, and a def naming
one becomes an unknown widget: laid out, not painted — exactly how a host older
than that def already behaves, so a slim build has no new failure mode to learn.
(`plane` is the one shared name: without `patcher`, a patcher node reads as the
scroll workspace it also is.) `midi` (live note painting) is default too;
`standalone` / `standalone-faust` (an embedded audio server) are not, because
they link the server crate.

Since CI only ever lints the default set, **`./check-features.sh`** runs the
matrix — clippy, rustdoc, the wasm gate and the suite over the floor, each
family alone, and both text and standalone builds — and `./check-wasm.sh` is the
browser-readiness gate it calls.

Two standalone prototypes double as demos of the rendering machinery:
`cargo run --release --bin waveform` (a navigable GPU waveform of a large
buffer) and `--bin spectrogram` (the STFT view sharing the same navigation).

## The widgets

The vocabulary, grouped; every widget, its props and its edit-back events are
specified in the [widget catalog](../../docs/gui-protocol.md#the-widget-catalog):

- **Containers & static** — `window`, `panel`, `label`, `text`.
- **Controls** — `slider`, `knob`, `number`, `button`, `toggle`, `menu`.
- **Live views** (read the server every frame) — `meter`, `scope`,
  `phasescope`, `spectrum`, `nodetree`, `canvas` (a script-supplied WGSL
  shader).
- **Signal views** — `waveform` and `spectrogram` (editor-grade, zoom/pan,
  linked navigation), `plot` (static measurement), `bpf` (a drawable
  break-point envelope, played by the server's own shape math).
- **Multitrack & graph** — `track` / `clip` (the multitrack editor's lanes),
  `pianoroll`, `graph` (a patcher of a bus-wired node graph).
- **Notation** — `score`, an engraved page the client sends as a display list of
  glyph outlines and primitives (the host reads no notation format), clickable
  and editable by the `xml:id` each primitive carries.

How the host is structured module by module — and the recipe for adding a
widget — is in the developer guide (below), not here.

## Documentation map

| What you want | Where it lives |
| --- | --- |
| The wire: `/gui_*` commands, the GuiDef document, props, edit-back events | [`docs/gui-protocol.md`](../../docs/gui-protocol.md) |
| Development: the host's structure, module map, how to add a widget | [`docs/architecture.md`](../../docs/architecture.md), "The GUI host" |
| Usage from Python: the arrangement and the multitrack editor | the [Python client book](https://clausters-python.readthedocs.io/), composition chapters |
| Crate API reference | `cargo doc --no-deps` |
| Roadmap | [`PLAN.md`](PLAN.md) |
