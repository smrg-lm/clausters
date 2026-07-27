# W4 — Components: the host's canvases in the document

Design for the milestone `clients/web/PLAN.md` labels W4. Approved 2026-07-27.

## What this milestone is

On the desktop, `clausters-gui` opens one window per GuiDef whose root is
`window`, and the system's window manager places, sizes and stacks them. In a
browser tab the drawing surface is a `<canvas>` in an HTML document, and the
document does the placing. That one substitution is the whole milestone:

> The host draws a GuiDef into a canvas. The canvas is an element of the
> document. **The document places it** — CSS, the order of the markup, the
> flow of the page — exactly as it places a paragraph or an image.

What that buys is not widget embedding as a trick: it is the desktop working
arrangement transposed onto a document. Canvases interleave with prose,
headings and images, so one page can be an interactive text with the instrument
sounding beside the paragraph explaining it, or an editing program whose panels
are laid out by the same CSS as everything else.

The second half of the milestone is how a component is *made*: a Python (later
Node) script builds the defs and the GuiDef with the ordinary client API and
writes a small directory — the **bundle** — that a page mounts with no client
library loaded at run time.

## Vocabulary

Kept distinct, in this document and in everything the milestone writes:

| Term | What it names |
|---|---|
| **window** | a desktop window of the native front. Never a browser thing. |
| `window` | the *root node type* of a GuiDef on the wire (`/gui_*`). Unchanged. |
| **component** | the custom element in the document, with its canvas. |
| **canvas** | its drawing surface — what the host renders into. |
| **bundle** | the directory of persisted data a component mounts. |

A component is written about as *mounted*, *resolved* and *freed* — the API's
own verbs. It is not "instantiated", "created" or "spawned".

## What the page loads, and what it does not

Running a component is the browser equivalent of running `clausters-gui
--standalone` on the desktop: **the host is the server's client, and there is
no scripting client in between**. So the page loads clausters — but clausters
here means the wasm engine and the wasm host, not the TypeScript client.

```
  authoring (python / node, the client API)  |  run time (the tab)
  -------------------------------------------|-------------------------
    SynthDef, signals, ugens                 |    the wasm engine
    the GuiDef builders                      |    the wasm host
    the writer that emits the directory      |    the element + the mount
              writes  ---->  the bundle  ---->  reads
```

This is what the bundle is *for*, and it has a concrete consequence:
`examples/piano/index.html` today imports `dist/index.js`, the whole package
facade, and so ships every def and GuiDef builder to a page that uses none of
them. W4 adds a **slim runtime entry**, `dist/runtime.js` — the engine, the
host, the element and the mount, and nothing else. The generated `index.js` of
a bundle imports that one.

A page that *does* want the TypeScript client (to sequence, to respond, to edit
live — the W2/W3 posture of `examples/gui-host.html`) imports it on top. Both
postures target the same element; the milestone delivers the first and only
makes sure the second can land in it.

## The three layers

```
  the HTML document      places: CSS, the order of the markup
        |
  the component (TS)     owns a canvas, a lifecycle, its attributes
        |
  the gui host (wasm)    N canvases, one `window`-rooted GuiDef in each
        |
  the engine (wasm)      one server, synthdefs
```

### The host: from one canvas to N

`clients/gui/src/host/web.rs` holds `window: Option<Arc<Window>>`,
`render: Option<WindowRender>` and `current_def: Option<i32>` — all singular,
with the comment "the browser shows one at a time". They become a map keyed by
def id: a wgpu surface, a size, a gesture state and a visibility flag per
canvas. The native front already keeps one surface per `window`-rooted GuiDef
(`host/gui/windows.rs`), so this is porting a model that exists.

Two things change direction while we are in there:

- **The element supplies the canvas.** Today `guiHost()` waits for winit to
  append a canvas to `<body>` and grabs whichever one is new — a page-wide
  singleton found by inspection. With winit's `with_canvas`, the component
  creates its own `<canvas>` and hands it over, which is both the correct
  ownership and the only way N of them can exist.
- **Size comes from the element.** A `ResizeObserver` plus `devicePixelRatio`
  on the component's box drives `resize(def_id, w, h)`. The host never reads
  the DOM.

The host learns nothing about HTML: it is told "this def draws into this
canvas, at this size, and right now it is (not) visible".

### Not rendering what is not seen

A document can hold fifty canvases with three in the viewport. The browser
already skips compositing what is off screen, but that does not stop *our* host
from computing the frame — the spectrum analysis, the scope advance, the FFT —
nor, more expensively, from keeping its `/c_stream` and `/tap_stream`
subscriptions alive, which is server CPU and wire traffic for something nobody
is looking at. (The same waste exists on the desktop behind an occluded
window.)

So each component carries an `IntersectionObserver` that tells the host when
its canvas leaves and re-enters the viewport; a hidden canvas is skipped on the
tick and its buses are dropped from the subscription set. It is a small,
self-contained item: if it tangles the event loop it can be dropped without
anything else moving.

### No window management

Components are placed in the markup by whoever writes the page, in the order
they want, the way web pages have always been made. Nothing opens, moves,
stacks or closes them. Mounting happens in `connectedCallback`, so an element
inserted later works; the deliberate omission is the *management* layer — an
element removed from the DOM, a `/gui_closed` travelling back — which is a
separate feature for whenever an editing program needs to open and close
panels.

## The bundle format

The existing native format is kept and extended in place, so one directory
still runs on all three legs (browser, `clausters-gui --standalone`, and a
loopback host against a running server).

```
fm-voice/
  index.js                          the generated ES module: registers the tag
  bundle.json                       the manifest
  defs/synthdefs/fm-voice.voice.json    a /d_recv payload, verbatim
  defs/graphdefs/fm-voice.graph.json    a /d_graph payload, verbatim
  defs/guidefs/fm-voice.json            the GuiDef record — a *template*
  presets/bright.json                   a param map
  boot.json                             optional, as today
  audio/hit.wav                         optional sample data
```

`bundle.json` grows from an enumeration into the component's contract, and
becomes a file **both** legs read (the native host reads it when present and
falls back to listing the directory when absent, so today's bundles keep
working):

```json
{
  "name": "fm-voice",
  "gui": "fm-voice",
  "synthdefs": ["fm-voice.voice", "fm-voice.trem"],
  "graphdefs": ["fm-voice.graph"],
  "widgets": 12,
  "symbols": {
    "nodes":   ["graph"],
    "buses":   [{ "name": "lfo", "rate": "control", "channels": 1 }],
    "buffers": []
  },
  "params": {
    "freq":  { "type": "float",  "default": 220.0, "min": 60.0, "max": 700.0 },
    "amp":   { "type": "float",  "default": 0.25 },
    "title": { "type": "string", "default": "FM voice" }
  },
  "presets": ["bright", "bass"],
  "buffers": { "hit": "audio/hit.wav" },
  "boot": true
}
```

### Two kinds of hole, one pass

The GuiDef record on disk is a **template** with two kinds of placeholder,
distinguished by sigil:

```
"@lfo", "@graph"     a symbol    — an id allocated by the page when mounting
"$freq", "$title"    a parameter — a value supplied by the tag, a preset,
                                   or the declared default
```

Widget ids are **not** symbols: the template numbers its widgets `1..N`
locally, and the resolver offsets them by a base the host allocates. Twelve
widgets would otherwise mean twelve placeholders for no gain, and the host
already recycles ids in blocks.

**Placeholders live only in the GuiDef record** (and in `boot.json`). That is
the invariant the format is built on, and it is what makes two mounted
instances cheap: the def payloads under `defs/` contain no holes, so they are
byte-identical between instances and are sent to the server once. It also
forces one authoring rule, which is the right rule anyway:

> A bus, a node or a buffer reaches a def **as a control**, never as a baked
> constant.

Today `piano_voice` does `out_ctl(0.0, env)` — the bus number is compiled into
the def — which is exactly why that bundle cannot be mounted twice. Written as
`out_ctl(control("env_bus"), env)` it can, and the mount passes the allocated
bus in the `/s_new`. The authoring API makes the wrong form hard to write.

Def **names** are prefixed with the bundle's name when the bundle is written
(`fm-voice.voice`), not at mount time — a name is a global namespace on the
server, and two bundles defining `voice` differently must not collide.

### Resolving, in Rust, in two pure steps

The logic is language-agnostic, so it lives in `clausters_core::bundle` and is
opened to the browser by `crates/clausters-core-web` and to Python by
`clausters-ffi`. Nothing is added to the `/gui_*` protocol and no state is
added to the host: what comes out of the resolver is the same
`/d_recv`/`/d_graph`/`/gui_def`/`/graph_new` traffic as today.

The caller allocates, so the resolver stays pure and only flat data crosses:

```
requirements(manifest)  ->  { widgets: 12,
                              nodes: ["graph"],
                              buses: [{ lfo, control, 1 }],
                              buffers: ["hit"] }

        ... the caller allocates from the page's own allocators ...

resolve(template, allocation, params)  ->  { def_id, tree, boot: [messages] }
```

`allocation` is `{ widget_base, nodes: {name -> id}, buses: {...},
buffers: {...} }`; `params` is the merged map. The resolver walks the tree and
the boot list, offsets every widget id, substitutes every `@` and `$`, and
type-checks each parameter against its declaration. Its errors are the useful
ones: an unknown symbol, a missing parameter with no default, a value outside
a declared range, a type mismatch.

`validate(manifest, template)` is the same machinery pointed the other way, for
the writers: both the Python and the (later) TypeScript writer call it before
emitting, so the two agree because they check against one schema rather than
because they were written carefully.

### Parameters and presets

Declared parameters become attributes on the tag. Resolution order is
**attribute → preset → declared default**, so a preset is a named bundle of
values and an attribute is a local override:

```html
<fm-voice></fm-voice>                      <!-- the defaults        -->
<fm-voice freq="440" title="voice 2"></fm-voice>
<fm-voice preset="bright" amp="0.1"></fm-voice>
```

A parameter reaches the synthesis through machinery that already exists — no
new mechanism. Either a widget carries it and its `bind` pushes it:

```python
knob(3, label="freq", value="$freq", bind=["/n_set", "@graph", "freq"])
```

or, when nothing draws it, the boot list does:

```python
boot=[["/graph_new", "fm-voice.graph", "@graph", 0, 0],
      ["/n_set", "@graph", "freq", "$freq"]]
```

### The generated module

The directory ships a five-line ES module so a page gets a named tag from one
import:

```js
// fm-voice/index.js — generated
import { defineComponent } from "/dist/runtime.js";
defineComponent("fm-voice", new URL(".", import.meta.url));
```

The runtime URL is an argument of the writer (defaulting to `/dist/runtime.js`)
because where the package is served from is the page's business, not the
bundle's. The generic element stays available for pages that would rather not
generate anything:

```html
<clausters-bundle src="./fm-voice" freq="440"></clausters-bundle>
```

## Mounting, in two phases

The host does not need audio; the engine does, and the autoplay policy will not
start an AudioContext without a gesture. With N components on a page, N power
buttons would be wrong — the AudioContext is page-wide. So the mount splits:

1. **On connect** — the component creates its canvas, allocates from the page
   allocators, resolves its template and opens the GuiDef on the host. The
   component draws immediately, as the reader scrolls to it.
2. **On the first gesture anywhere in the page** — the engine's context
   resumes and every mounted component's server half is sent: its defs (once
   per def name per page, deduplicated), its buffers, and its boot list.

Until then a component shows its power affordance over the canvas; `<clausters-
power>` keeps working as the page-wide switch, and any component's affordance
serves as the gesture for all of them.

Failures are per component and stay local: a component that cannot fetch or
resolve its bundle shows the error on itself and emits `clausters-error`, and
the rest of the page comes up. `clausters-ready` fires per component with its
resolved def id.

## The authoring API (Python)

`clausters.bundle` — a writer over the builders that already exist. It holds
the symbol table so the author names things instead of numbering them, and
writes the directory plus its module:

```python
b = Bundle("fm-voice", title="FM voice")
b.param("freq", float, default=220.0, min=60.0, max=700.0)
b.param("amp", float, default=0.25)

lfo   = b.bus("lfo", rate="control")     # -> "@lfo"
graph = b.node("graph")                  # -> "@graph"

b.synthdef(voice())                      # named "fm-voice.voice"
b.graphdef(graph_def())
b.gui(scene(lfo, graph))
b.preset("bright", freq=660.0, amp=0.15)
b.write("./fm-voice", runtime="/dist/runtime.js")
```

`b.bus(...)` returns the placeholder string, so it reads naturally where a bus
index goes (`meter(4, lfo)`), and a def that takes it does so through a
control. `write()` validates through the core before emitting, so a bundle that
would fail to mount fails to be written.

TypeScript gets the same writer, under Node, after Python — the reference
client leads and the port is mechanical, which is the repo's standing rule.

## Testing

- **Rust, `clausters_core::bundle`** — the substance of the milestone's logic
  is here, so this is where the cases live: widget-id offsetting through nested
  trees, `@`/`$` substitution in props and in boot messages, parameter typing
  and range checks, preset-over-default and attribute-over-preset merging, and
  each error (unknown symbol, missing parameter, bad type, out of range).
- **Rust, the host** — two defs attached to two canvases render and receive
  gestures independently; a hidden canvas is skipped on the tick and drops its
  buses.
- **`node --test`** — the manifest schema round-trips; the Python writer's
  output matches a frozen vector (the `def-parity`/`gui-parity` pattern); and
  the emitted `dist/runtime.js` module graph does **not** reach `defs/`,
  `gui/guidef.ts` or `seq/` (the slim-entry invariant, asserted rather than
  hoped for).
- **Headless Chrome** — the milestone's acceptance page: two instances of one
  bundle plus one of another, interleaved with prose. All three canvases draw;
  the two instances of the same bundle got different buses and node ids and
  their def was sent once; `freq="440"` on one and the default on the other are
  audibly and queryably different; a component scrolled out of view stops
  streaming; and no component's failure takes the page down.
- The existing W0–W3 suites and the four acceptance pages stay green.

## Documentation

- `docs/clients.md` — "A standalone bundle in a tab" grows into the component
  format: the manifest, the placeholders, the two-phase mount.
- `docs/architecture.md` — the host's canvas-per-def structure.
- `docs/decisions.md` — three entries worth the space: *the document places,
  the host draws* (why the browser front has no window management); *holes live
  only in the GuiDef record* (why def payloads stay shareable, and the control
  rule that follows); and *the run-time entry excludes the authoring builders*
  (why the package has two entry points).
- `clients/web/README.md` and `BUILD.md` — writing and serving a component.
- Examples: `piano` and `graph-controls` ported to the writer, plus a new
  `examples/document/` — an interactive text, which is also the acceptance
  page's shape and the form the two mdBooks can embed later.
