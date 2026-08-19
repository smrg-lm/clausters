# Clients and language bindings

Clausters is a server; clients drive it. This chapter is the **cross-language
map**: the one native contract every client sits on, the Python client built on
it, the TypeScript client that sits on the same contract, and the path to
distributable packages. The
client work lives in the `clients/` tree; this is the architectural overview.

## One contract: the C ABI

Everything a client needs that must be *native* lives behind a single,
versioned **C ABI** — the scsynth plugin-ABI lesson: every binary boundary is
versioned and checked. Two cdylibs expose it, and the boundary rule is the same
for both: **only flat data crosses** — `f32`/`f64`/integers, pointer+length
arrays, NUL-terminated error strings. Never a library type (a numpy array can
*view* a returned pointer, but that is the client's choice, not a dependency).

| cdylib | crate | what it is | key entry points |
|---|---|---|---|
| `libclausters_ffi` | `clausters-ffi` over `clausters-core` | the **shared numeric/timing core** | `clausters_core_abi_version`, `clausters_core_unary`/`_binary` (builtins), `clausters_core_whitenoise`, `clausters_core_beats_to_secs`/`_secs_to_samples`/…, `clausters_sched_*` (beat queue), `clausters_clocksync_*` (sample-clock model), `clausters_rng_*` (seeded value stream), NTP timetag packing, `quant_delay`, `degree_to_midinote` |
| `libclausters` | `clausters` (feature `embed`) | the **server as a library** | `clausters_abi_version`, `clausters_render` (offline), `clausters_open`/`_send`/`_poll`/`_clock`/`_ctl_*` (in-process live server) |
| `libclausters_midi` | `clausters-midi` over `midly`/`midi2`/`midir` | **MIDI I/O** | `clausters_midi_write_smf` (`.mid`), `clausters_midi_write_clip` (MIDI 2.0 clip), `clausters_midi_free`; with `--features live`: `clausters_midi_output_open`/`_send`/`_close` (virtual port) |

Beside the in-process embed path, the same OSC reaches the server over **UDP**
or **shared memory** (`--shm`); see [Local transports & embedding](ipc.md). So a
client has three ways to talk to the server (UDP, shm, embed) and one way to
reach the native core (`libclausters_ffi`) — all language-agnostic.

Why a shared core at all: the builtins, the seeded white noise and the
beat/second/sample math are compiled **once** in `clausters-core` and used by
both the server's UGens and every client, so client-side results match the
server **by construction** for the operations the server computes natively.

The rule extends to everything a client computes about **values or time**, so a
port rebinds the core instead of reimplementing behaviour. Concretely, the core
owns: the clock's beat-ordered **scheduler queue** (min time, stable insertion
order for ties), the **sample-clock tracking model** (the least-squares
`sample = a + b·t` fit behind locking a clock to a server over the network),
the **seeded value stream** the random patterns draw from (one `u64` state
word, so a seeded sequence replays identically in every client language — a
host language's own RNG never leaks into sequenced values), **NTP timetag
packing** (one rounding rule, identical bits for identical instants),
**quantization** (`quant` boundary snapping) and the **pitch-space math**
(scale degree → MIDI note). What each language keeps is its control flow (the
coroutine driver that resumes routines) — and, as the one documented
exception, the **OSC byte codec**: structured message arguments cannot cross a
flat C ABI, so encode/decode of the wire bytes stays per-language while every
time value inside them comes from the core.

## The Python client

`clients/python/` is the reference client, a selective port of
SuperCollider's class library (sc3) covering both def formats (FaustDefs and
UGen-graph SynthDefs). It is **pure Python at runtime**: it
reaches the core through `ctypes` over `libclausters_ffi`, and speaks ordinary
OSC bytes to the server (UDP, TCP, or shm/embed via the transport module). It
mirrors the native contract in three layers — `base` (server-agnostic timing
and values), `seq` (events and patterns) and `defs` (the Faust/UGen definitions
and the `Server`, whose swappable interface is the live-RT / offline-NRT seam).

It has its **own documentation** — a guide and the generated API reference:
**[the clausters Python client book](https://clausters-python.readthedocs.io/)**.
<!-- Cross-link to the companion book; update the URL if the Read the Docs slug differs. -->

**This is also the proof the contract is language-agnostic**: Python — a
non-Rust language — already drives the whole system (core math, offline render,
live server) purely through the C ABI and OSC. Nothing in the boundary is
Python-specific.

## The GUI host: a scriptable peer

The GUI (`clients/gui`) is **another peer in the system, not code compiled into
the audio server** — the same decoupling Clausters already uses for audio. Just
as SuperCollider's `sclang` builds Qt widgets by sending messages to a separate
widget engine, a **GUI host** process owns the windows, widgets and the GPU and
speaks a widget protocol; scripts (Python now, JavaScript later) send it widget
commands and receive interaction events, while the audio server is untouched.
The host plays two roles in one process: a GUI server for the languages and a
*client* of the audio server (it reads buffers, control buses and the node tree
and can send control back). A bound widget can forward its value straight to the
audio server, bypassing the script, for low-latency control.

Two design choices carry the rest:

- **Declarative, def-shaped protocol.** A whole widget tree is one document, not
  a stream of per-widget messages — mirroring `SynthDef`/`GraphDef`. `/gui_def
  <id> <json>` builds a tree in one message (JSON as payload, OSC as framing,
  the single `osc::decode_packet` door), `/gui_set` updates a live widget,
  `/gui_free` frees a subtree. The wire form is generic (`{id, type, props,
  children}`) so the protocol never changes when a widget type is added; an
  unknown type is laid out but not painted, so old and new hosts interoperate.
- **A web-capable GPU substrate, one stack for both targets.** The heavy widgets
  (waveform, spectrogram, scopes) are custom GPU rendering written against
  `wgpu`/WGSL, which runs natively today and under WebGPU in a browser unchanged
  — so the native desktop host comes first and the browser target is reached by
  swapping the surface, not forking the renderers. The heavy views follow one
  rule — **never resolve the signal finer than the screen**: work is bounded by
  `samples_per_px`, and the expensive analysis (a peak pyramid, an STFT) is
  treated as a cache that moves through local shared resources (mmap natively,
  fetch in the browser), never chunked over the wire.

The wire reference — the commands, the GuiDef document, the events and the widget
catalog — is [The GUI protocol](gui-protocol.md). The crate's front door — a
quick start, the widget vocabulary at a glance and its documentation map — is
[the `clients/gui` README](https://github.com/smrg-lm/clausters/blob/main/clients/gui/README.md).

The widget catalog runs from the ordinary controls (labels, knobs, sliders,
numbers, buttons, toggles, text, menus) through the live meters and scopes
(`meter`, `scope`, `phasescope`, `spectrum`, `nodetree`) to the editor-grade
views: the heavy `waveform` and `spectrogram` (multichannel lanes, adaptive
rulers, a draggable selection, a playhead tracking the engine clock, linked
navigation groups), the drawable `bpf` envelope editor, a static `plot`, a
shader `canvas`, an engraved `score` page — and the **composition** views below.
Their reference is the Python builders' documentation, since that is how a
script names them.

The `score` is worth a note, because it is the one widget whose samples the
host cannot read. A client engraves music notation (through
[verovio](https://www.verovio.org/) and the shared notation layer, both shipped
inside the wheel) and sends a **display list**: glyph outlines keyed by SMuFL codepoint, plus the placed
glyphs, staff lines, stems, beams, slurs and text, in page units. The host fits
that into the widget and tessellates it into the same triangle mesh as the rest
of the chrome — it never parses MEI, MusicXML or any notation format. Every
primitive carries the `xml:id` it was engraved from, so the page is interactive
without the host understanding it: a click reports the element under the cursor,
a drag reports a pitch edit in diatonic steps, and the client — which does own
the score — applies it, re-engraves and sends the page back. A cursor track
engraved beside the drawing lets the page follow playback off the engine clock,
like the timeline views.

### The composition views: a multitrack editor and a patcher

The newest arc of the GUI is a **DAW-style multitrack editor**, and it exists to
put a *client-side arrangement model* on screen: a `track` is a lane, a `clip`
is a placed rectangle spanning `[offset, offset + dur]` on a time axis the lanes
of a window **share** (they zoom and pan as one navigation group, and the axis
spans the composition). A clip's body is one of three, and the choice is the only
thing that differs between them:

- a **take** — a server buffer, a mapped file or a prebuilt peak cache, decimated
  to the clip's pixel width through the shared peak pyramid, so a minutes-long
  clip costs a screen's worth of columns and never rides the wire as JSON;
- a **piano-roll** of note events (time and pitch);
- an **automation curve** — the same `curve` element that stands on its own,
  placed on the lane and editable in place, evaluated through the same
  envelope-shape math the server's `EnvGen` plays.

Everything is editable back: dragging a clip or its edge emits `"clip"`, dragging
a break-point emits `"points"`, and the script's model — not the widget tree — is
what those events change. A **logical** group (members wired to each other through
buses, the shape a `GraphDef` expresses) is not a timeline at all, so it draws as
a `patch` **patcher** instead: directed, typed boxes with inlets on top and
outlets on the bottom, and a cord per `outlet -> inlet` connection (the buses are
not drawn — a cord *is* a bus). Direction is structural, read from the def (a
control feeding an `In` is an inlet, one feeding an `Out` an outlet), so dragging
an outlet onto an inlet draws the cord (see [Design decisions](decisions.md)).

The Python side of all this is `clausters.gui.Editor`: it draws a composition
into that window, applies the edit-backs onto the arrangement, and re-renders it — so
the graphic is not a picture of the music, it *is* the music. Its user
documentation is the composition chapter of the Python client's book.

**Playing any of these views is one shared object**, not a per-view transport:
`clausters.gui.Transport` drives a playhead over the samples and the view's line
together, and every view — a lane, a piano-roll, an engraved page — uses that one.
What a view contributes is a single conversion (its cursor's unit: timeline
samples for a lane, score milliseconds for a page); everything else is the same
two numbers the host already understands. A port keeps that shape: the anchor
arithmetic is small, but splitting it per view is how a client ends up with three
transports that disagree about the end of a piece.

## The GUI host in the browser

The GUI host (`clients/gui`) also compiles to **WebAssembly** and runs in a browser tab: the same widget protocol, layout, renderers and interaction as the desktop host, over a `<canvas>`. It renders through **WebGPU where the browser truly supports it and WebGL2 otherwise** (~99% browser reach), and it talks to an audio server over one of two legs: a **separate server over WebSocket** (start it with `--ws`, default port 57120), or the **in-page engine** — the audio server itself compiled to wasm, running inside an AudioWorklet on the same page (`GuiBridge.connect_page`; see the standalone quick start below). No server process is required in the second case.

The browser fills the host's data paths over the network instead of shared memory and mapped files:

- **Meters/scopes/canvas buses**: the host subscribes the buses its widgets read with `/bus_stream` and the server streams `/bus_stream.reply` snapshots back at ~30 fps over the same WebSocket — the network counterpart of the shared-memory segment (see the control-bus commands in [Def schemas](schemas.md)).
- **Audio-bus views**: the host subscribes the audio buses its audio-rate scopes, phasescopes (a stereo pair) and live spectra read with `/bus_tapStream` and the server streams `/bus_tapStream.reply` windows back — the network counterpart of reading the segment's sample rings, and the subscription is itself what asks the server to record those buses (see [Def schemas](schemas.md)). The phasescope's correlation and goniometer geometry and the spectrum's FFT all come from `clausters-core`, so the browser computes them in wasm identically to the desktop.
- **Bulk waveform/spectrogram/plot/clip data**: a `path`/`cache` reference is fetched as a URL against the page origin (raw `f32` samples — every interleaved channel kept — a prebuilt peak-pyramid cache, or an STFT cache; the pyramids and STFT lanes for raw fetches are built in wasm — the analysis lives in `clausters-core`), and a server `buffer` reference is pulled over `/buffer_query` + chunked `/buffer_getRange` on the WebSocket leg. The editor chrome of the two heavy views (multichannel lanes, adaptive time/Hz rulers, the selection overlay, the vertical `y_start`/`y_len` view window) renders through the same shared frame path as the desktop — a `/gui_set` of any of it displays identically in the browser, and the pointer/wheel/keyboard gestures (drag-select, pan, zoom, BPF and clip editing, the piano-roll editing set) ride the same shared gesture machine as the desktop, so an edit behaves identically on either front; the playhead is driven by polling `/clock_query` once per animation tick instead of reading the shared segment's sample clock. The **linked navigation groups** (an explicit `link` prop shares one horizontal view, selection and playhead across timeline views; `/gui_set` of `view_start`/`view_len`/`sel_*`/`playhead_at` on any member applies group-wide) live in the host core's protocol dispatch, so they behave identically in the browser.

**Quick start** (from `clients/web/`, the one web directory — the host's wasm bundle is staged into the package's `dist/gui-host/`; one-time setup: `rustup target add wasm32-unknown-unknown` and `cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>`):

```sh
./build.sh                # the wasm builds + wasm-bindgen, staged into dist/
python3 -m http.server    # then open http://localhost:8000/examples/gui-host.html
```

The page loads the bundle and calls the wasm entry point `start()`, which returns the **binding surface** (`GuiBridge`) the page drives: `def(id, json)` feeds a GuiDef (the same JSON the Python builders emit), `feed(packet)` pushes any raw `/gui_*` OSC packet, `poll()` drains the outbound `/gui_event`/`/gui_info`/`/gui_closed` packets, and the audio-server leg attaches with `connect_server(url)` (a `--ws` server) or `connect_page(send)` (the in-page engine: outbound packets go to the `send` callback, replies come back through `server_reply(packet)`; a `bind`-ed widget forwards straight to it either way, with no script round-trip). That surface is the *binding*, not the client: a page programs against the TypeScript client's `GuiHost` (below), which wraps it — `clients/web/examples/gui-host.html` is that client driving the host, with the bound and the scripted control paths side by side.

### The component bundle: an instrument as an element of the document

A **bundle** is the persisted form of an instrument — its def payloads, its
GuiDef, its presets, its samples — and the manifest that says what mounting it
needs. One directory runs on three legs: a browser tab, the desktop
(`clausters-gui --standalone <name> --data-dir <dir>`), and a loopback host
against a running server. In the tab the engine runs in an AudioWorklet
([Using as a library](using-as-a-library.md) describes the pulled server it
wraps), the GUI host draws into a `<canvas>`, and the streamed data paths
(`/bus_stream`, `/bus_tapStream`, `/buffer_getRange`, `/clock_query`) ride the in-page leg
unchanged — meters, scopes and buffer views are live with no server process
anywhere.

On the desktop `clausters-gui` opens a window per `window`-rooted GuiDef and the
window manager places it. In a tab the drawing surface is an element, and **the
document places it** — CSS, the order of the markup, the flow of the page. So
canvases interleave with prose and images, and one page can be an interactive
text with the instrument sounding beside the paragraph that explains it
(`clients/web/examples/document/`).

```
fm-voice/
  index.js                            the generated ES module: registers the tag
  bundle.json                         the manifest
  defs/synthdefs/fm-voice.voice.json  a /def_send synth payload, verbatim
  defs/graphdefs/fm-voice.graph.json  a /def_send graph payload, verbatim
  defs/guidefs/fm-voice.json          the GuiDef record - a *template*
  presets/bright.json                 a parameter map
  audio/hit.wav                       optional sample data
```

**Two kinds of hole.** Mounting the same bundle twice on one page must not
collide, so the GuiDef record is a template with placeholders, told apart by
sigil: `"@lfo"` is a **symbol** — an id the page allocates (a node, a bus, a
buffer) — and `"$freq"` is a **parameter**, a value the tag supplies. A doubled
sigil escapes it (`"$$5"` is the literal `"$5"`). Widget ids are deliberately
not symbols: the template numbers its widgets locally and the mount offsets
them by an allocated base, so twelve widgets do not mean twelve placeholders.

Placeholders live **only** in the GuiDef record (and in its `boot` list). That
is the invariant the format rests on: the def payloads under `defs/` hold no
holes, so they are byte-identical between two instances and are sent to the
server once. It forces one authoring rule, which is the right rule anyway — *a
bus, a node or a buffer reaches a def as a control, never as a baked constant*
— and the writer refuses to emit a bundle that breaks it.

`bundle.json` declares all of that:

```json
{
  "name": "fm-voice",
  "gui": "fm-voice",
  "synthdefs": ["fm-voice.voice"],
  "graphdefs": ["fm-voice.graph"],
  "widgets": 12,
  "symbols": {
    "nodes":   ["graph"],
    "buses":   [{ "name": "lfo", "rate": "control", "channels": 1 }],
    "buffers": []
  },
  "params": {
    "freq":  { "type": "float",  "default": 220.0, "min": 60.0, "max": 700.0 },
    "title": { "type": "string", "default": "FM voice" }
  },
  "presets": ["bright"],
  "buffers": { "hit": "audio/hit.wav" },
  "boot": true
}
```

`"widgets"` is the **width of the id block** an instance needs (the highest
local widget id, the root's included), not a count — a template may number
sparsely. Every field but `"gui"` has a default, so a bundle written before the
contract existed still mounts: it declares nothing, and nothing is substituted.

**Resolving** is two pure steps with the allocation in between, shared by every
leg (`clausters_core::bundle`, opened to the browser by `clausters-core-web`
and to Python by `clausters-ffi`):

```
requirements(manifest)  ->  { widgets, nodes, buses, buffers }
        ... the caller allocates from its own allocators ...
resolve(manifest, template, allocation, params)  ->  { def_id, tree, boot, params }
```

Nothing is added to the `/gui_*` protocol and no state to the host: what comes
out is the same `/def_send`/`/gui_def`/`/graph_new` traffic as a
hand-written bundle. `validate` is the same machinery pointed the other way,
for the writers.

**Parameters and presets.** Declared parameters are attributes on the tag, with
`preset` beside them; resolution is **attribute → preset → declared default**,
each value typed and range-checked at mount:

```html
<fm-voice></fm-voice>                      <!-- the defaults -->
<fm-voice freq="440" title="voice 2"></fm-voice>
<fm-voice preset="bright" amp="0.1"></fm-voice>
```

A parameter reaches the synthesis through machinery that already exists: a
widget carries it and its `bind` pushes it, or the `boot` list does.

**Mounting is two phases**, because the host does not need audio and the engine
does — and the AudioContext is page-wide, so N power buttons would be wrong.
On connect the component allocates, resolves and opens its GuiDef: it draws as
the reader scrolls to it, with no gesture. On the first gesture anywhere on the
page every mounted component's server half goes out — its defs (once per
payload for the page), its samples (once per URL) and its `boot` list. Failures
are per component: one that cannot fetch or resolve its bundle shows the error
on itself and emits `clausters-error`, and the rest of the page comes up.
`clausters-ready` fires per component with its resolved def id.

**Unmounting is one.** An element removed from the document frees its window
and widgets (`/gui_free`), the nodes its `boot` instantiated (`/node_free`) and
its canvas, and returns its widget, node and bus ids to the page's pools — so a
document that adds and removes instruments holds a flat occupancy. What is
shared stays: the AudioContext, the host, and the def payloads and sample
buffers, which are the same data for every instance of a bundle. An element
connected again mounts afresh over a new allocation rather than resuming, and a
window closed by the host (`/gui_closed`) reaches the element that mounted the
def, which unmounts and emits `clausters-closed`.

**What is not loaded.** Running a component is the browser equivalent of
`clausters-gui --standalone`: the host is the server's client and there is no
scripting client in between. The builders ran in the authoring script; what the
page fetches is data. So the generated module imports `dist/runtime.js` — the
engine, the host, the OSC codec and the mount — and not the def builders, the
GuiDef builders or the sequencing layer (`clients/web/tests/runtime-graph.test.ts`
asserts the exclusion). A page that *does* want the TypeScript client imports
`dist/index.js` on top; both postures target the same element.

**Writing one** is `clausters.bundle.Bundle` in the Python client, which holds
the symbol table so the author names things instead of numbering them, and
validates through the core before emitting — an unmountable bundle is
unwritable. The generic `<clausters-bundle src="./fm-voice">` mounts a bundle
without a generated module. `clients/web/examples/piano/` and
`examples/graph-controls/` are the worked examples;
`clients/web/tests/components.html` is the acceptance.

## The TypeScript client (started)

The browser-first TypeScript client grows inside the same `clients/web/`
package (roadmap: `clients/web/PLAN.md`), and has its own book — the mdBook in
`clients/web/docs/`, the third of the repository's three, with its API
reference generated from the sources' TSDoc:
**[the clausters web client book](https://clausters-web.readthedocs.io/)**.
Four layers are in place.
<!-- Cross-link to the companion book; update the URL if the Read the Docs slug differs. -->

**The seam.** The **OSC codec through the shared core**
(`crates/clausters-core-web`, a thin wasm-bindgen shell over `clausters-core`,
staged as `dist/core/` — byte-identical to the server and the Python client,
held by committed parity vectors generated from the Python codec) and the
**carrier seam** (`Connection`): `WsConnection` to a `--ws` server and
`pageConnection()` over the in-page engine, one interface, so everything built
above never names a transport.

**The client.** `Server` and the def model, mirroring the Python client's
`clausters/defs/` module for module: both def families as peers (`SynthDef`
with the lowercase UGen callables, `FaustDef` with the Faust signal API) plus
`GraphDef`, the `/server_sync` barrier, nodes, groups, graph instances, buses,
buffers and the introspection queries. The allocators come from the same core
— the wasm shell also exposes the **registry** (the occupancy map behind node
ids, buses and buffers) that `clausters-ffi` exposes to Python, and a `Server`
sizes itself from the server's own `/server_query`. Two shapes differ from the
reference client by necessity, both recorded in `docs/decisions.md`: everything
that waits is a **promise** (the browser has one thread), and the graph
composes **by method** (`sine(freq).mul(amp)`), TypeScript having no operator
overloading — so parity is asserted on the **emitted spec JSON**, against
vectors frozen from the Python builders, rather than on the source.

A Faust def reaches a **native** server only: the in-page engine is the
`synth,embed` build with no LLVM JIT. That is a property of the build, not of
the client — nothing above the seam names a carrier.

**The sequencing.** The clock, routines, events, patterns and timelines,
mirroring `clausters.seq`. As in Python, no time formula and no random value is
computed in the client's own language: the beat-ordered queue, the
beat↔second↔sample arithmetic, the bundle timetags, the seeded RNG and the
builtins are the core's, through the same wasm shell; what is TypeScript is the
coroutine driver (`function*`/async in place of Python's `yield`) and the
composition of events and patterns. Two timebases are available — the
monotonic clock and the Web Audio sample-clock — and the driver stays on the
page: only the wake-up moves to a shared tick worker, which is how the browser
buys the property Python gets from a background thread.

**The input path.** `OscFunc`, mirroring `clausters.responders` — the same
constructor, the same `(msg, time, src)` callback, the same `argTemplate` and
`oneShot`. What differs is the **receiver** under it: the reference client binds
a UDP port any application can target, and a page can bind nothing, so a
receiver wraps the `Connection` the client already has and `src` names a carrier
(a socket's URL, or `page`) rather than a `(host, port)` pair. Each `Server`
carries one (`server.receiver`), which is also the default a responder resolves
through the ambient session — and the door the client's own reply handling goes
through, so what a page matches and what the client waits for arrive the same
way. MIDI responders are not ported: in a browser both MIDI directions are one
API (Web MIDI), so they land together.

**The GUI.** `GuiHost` and the GuiDef builders, mirroring the Python client's
`clausters/gui/`: the widget catalogue as functions (`gui.window`, `gui.knob`,
`gui.waveform`, `gui.track`, …) emitting the same JSON document, the same
client-side id allocation out of one recycling namespace, and the same
name-addressed handles (`win.widget("cutoff").set({ value: 800.0 })`). It sits
on the very same `Connection` seam the audio client does, so the two carriers a
browser has are one line apart: `GuiHost.page()` drives the wasm host on this
page's canvas through its binding bridge, `GuiHost.connect(url)` a native
`clausters-gui --ws` host. A widget is **bound** the same way
(`w.bind("/node_set", node.id, "freq")`), and the value then flows host → engine
with no round trip through the page's script. Two differences from the
reference client, both in `docs/decisions.md`: the options are camelCase where
the props are the wire's snake_case, and there is **no pump** — the host's
`/gui_event`/`/gui_closed` arrive as handle callbacks, `query` as a promise.

The package mirrors the Python client's structure (`src/base`, `src/defs`,
`src/gui`, … with `examples/` and `tests/` beside them) and the toolchain is
deliberately minimal — `tsc` type-checks and emits `src/` to `dist/` (plain
ES modules plus declarations and source maps), tests run from source under
`node --test` with no runner package; see `clients/web/BUILD.md`.

This is the JavaScript client: there is no second one planned. Running the
package **outside the browser** — a node WebSocket carrier for headless
scripting, the way `clients/python` runs with no display — is a future
direction in that roadmap, and it needs no new native work either: the same
package over the same OSC, against a native server.

## Distribution

- **Python (done)**: a platform-tagged **wheel** that bundles the cargo-built
  cdylibs (`libclausters_ffi`, `libclausters` with `embed,realtime` and
  `libclausters_midi` with `live`) inside
  the package (`clausters/_libs/`), so an installed package is self-contained —
  no `target/` directory, no build step at import. The runtime stays
  stdlib-only; the loaders prefer the bundled copy, falling back to the
  workspace `target/` in a source checkout. A `setup.py` build hook runs `cargo
  build` and stages the libraries; `python -m build --wheel clients/python`
  produces the wheel. See the [Python client
  book](https://clausters-python.readthedocs.io/) for the install recipes and
  the env knobs (`CLAUSTERS_WORKSPACE`, `CLAUSTERS_CARGO_FEATURES`, …). The
  wheel is **Faust-enabled**: it bundles libfaust and the libLLVM it JITs with
  beside the cdylibs, so a `FaustDef` compiles on a machine with neither
  installed. Cross-platform CI wheels (cibuildwheel / manylinux) are still
  future work.
- **TypeScript / web (published on npm)**: `npm install clausters`. The package
  is the client's `dist/` — plain ES modules plus the wasm the core, the engine
  and the GUI host compile to, staged beside them, so an install needs no
  toolchain and fetches nothing at run time. No per-platform native addon: the
  browser build is wasm, and a native server is reached over WebSocket. The
  release tag publishes it beside the wheel, through a checker
  (`npm run check-package`, which `prepublishOnly` runs) that refuses a `dist/`
  missing its wasm bundles or a version out of step with the crate's.
- **Reproducible Faust build (done for native/CI/release)**: the `faust` feature
  needs libfaust built with the LLVM backend. It is now vendored under
  `third_party/` — `faust.pin` (the exact commit + LLVM version) and
  `build-faust.sh` (one recipe: fetch, build, install, stage libLLVM) — so local
  dev, CI and the release wheel produce a *deterministic* bundle instead of
  whatever the build host had. The npm package needs no such build: its wasm
  engine carries no LLVM JIT, so Faust is not part of it — a `FaustDef` is
  compiled by a native server reached over WebSocket.

## Status at a glance

| Piece | State |
|---|---|
| Shared core + C ABI (`clausters-core`/`clausters-ffi`) | done |
| Python client (`base`/`seq`/`defs`, incl. UGen `SynthDef`) | done |
| Cross-language docs + sequencing example | done |
| Python wheels packaging | done |
| MIDI interfaces in the Python client (`MidiServer`, SMF / MIDI 2.0 clip export, live port) | done |
| Client-side OSC/MIDI responders (`OscFunc`/`MidiFunc`) | done |
| Browser GUI host (wasm bundle; meters over `/bus_stream`, bulk over fetch/`/buffer_getRange`) | done |
| Arrangement model in the Python client (elements, recursive groups, rendering) | done |
| Multitrack editor + patcher (tracks/clips, piano-roll, automation curves, `patch`) and the driver that binds them to the arrangement | done |
| Engraved music notation (the `score` widget, its display list and the click/transpose edit round trip) | done |
| Notation layer in the shared core (`clausters-notation` + `clausters_core::notation`, over the C ABI; every client a shell) | done |
| Reproducible `third_party` Faust and verovio builds (pin + script; native/CI/release) | done |
| TypeScript/web client (the core over wasm, `Server` + both def families, the GUI, the sequencing layer, the OSC responders, the document's components) | done |
| npm packaging of the web client (the tarball, its contents checked) | done |
| Web client documentation book (mdBook + generated API reference) | done |
| Publishing the web client (npm registry, Read the Docs project) | done |
