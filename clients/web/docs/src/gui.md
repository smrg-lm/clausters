# The visual elements in a page

The GUI host is the same host the desktop runs, compiled to wasm and drawing on
a `<canvas>` in your document — so the widget tree, the props, the events and
the bindings are the ones the [Python client's GUI
chapter](https://clausters-python.readthedocs.io/en/latest/gui.html) walks
through, and that page is the tutorial. This one says what the browser makes
different, and nothing that book already says.

**The host is what draws, always** — and in a page that is worth saying twice,
because a page has a `<canvas>` of its own and the temptation is right there.
A script names what to look at (`plot`, `scope`, a `waveform`/`scope`/
`spectrum` widget) and the host reads the bus or the buffer and paints it; this
client computes no picture — no pixel column, no trigger, no decibel curve —
and neither does the Python one. The two are one client in two languages, and a
figure drawn two ways is a difference nothing checks. Drawing your own canvas
over the [data paths](data.md) is of course allowed, and it is *your* program:
not a surface this package provides, documents or keeps in step.

## The model, in one table

A GuiDef node is `{id, type, props, children}`, and a `type` names one of three
things:

| Kind | What it is | Types |
|---|---|---|
| **Container** | owns 0, 1 or 2 **axes**, and so a coordinate system its children are placed in | `window`, `layout`, `plane`, `field` |
| **Element** | draws against the axes of the container holding it | `signal`, `notes`, `curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label` |
| **Control** | an element with a value and no axis | `slider`, `knob`, `number`, `button`, `toggle`, `text`, `menu` |

The builders named after the old catalog — `panel`, `stack`, `scroll`,
`waveform`, `plot`, `scope`, `spectrum`, `spectrogram`, `phasescope`, `track`,
`clip`, `timeruler`, `pianoroll`, `bpf`, `piano`, `nodetree`, `patch` — are
**shortcuts** onto those nodes with the props of one common case. `layout`,
`plane`, `field` and `signal` are the general ones beside them. Both emit the
same JSON the Python builders emit, vector for vector; the wire is the shared
thing, the language surface is not.

## What the browser changes

**camelCase options, one bag.** Where Python takes keyword arguments in
snake_case, the TypeScript builders take one options object in camelCase, and
the children come after it:

```ts
import { gui } from "clausters";

gui.view(
    { title: "filter", w: 520, h: 260, flow: "col" },
    gui.label("a filter", { h: 24 }),
    gui.panel(
        { flow: "row" },
        gui.knob({ name: "cutoff", label: "cutoff", min: 20.0, max: 20000.0, value: 800.0 }),
        gui.knob({ name: "res", label: "res", min: 0.0, max: 1.0, value: 0.3 }),
    ),
    gui.signal({
        name: "wave",
        view: "trace",
        path: "take.f32",
        navigable: true,
        axes: { x: { unit: "beats", tempo: 2.0, link: 1 }, y: { unit: "db" } },
    }),
);
```

`textSize` becomes `text_size` on the wire, `baseBucket` becomes `base_bucket`,
`viewZoom` becomes `view_zoom`. Inside an `axes` pair the keys are the wire's
own (`sample_rate`, `sel_start`), since that object goes through untouched.

**A builder returns a `View`, and the view opens itself.** The tree is the
subject of the sentence — `view(...).open()` rather than `host.open(view(...))`
— the way a def is (`synthdef.send(server)`), and there is one root builder,
not two: a view with a parent is a component, a view with **no** parent is the
window, and any node opens (`knob({...}).open()` is a window that is a knob).
The document is unchanged: a `View`'s own properties are the JSON, so
`JSON.stringify` writes what it always wrote. What is added is the name index
(`v.find("cutoff")`, `v.names()`) and `open`; the bracket stays the document
key, because the two addressings cannot share one. `window` is the older
spelling of `view` and still works.

**No widget carries an id.** You pass `name` instead, and `open` hands back a
window handle you index by name. The ids are allocated **in the document that
goes out**, never written into the tree you wrote — which is what lets one view
open as many times as you like, each window with ids of its own, and what makes
the same subtree nested twice two widgets rather than one.

**The element goes in `open`, and a view that names none gets a canvas of its
own.** `view(...).open(el)` mounts the view into that element's box — the canvas
inside it is made and fitted for you, so `attach`/`fit` stop being something a
script writes — and `view(...).open()` appends a canvas to the document. That is
how *a view with no parent is a window* finishes in a page: **a view with no
element is a canvas**, so a document holds as many as it opens, and
`win.canvas` is the one this window draws on. A host reached over a socket has
windows of its own and refuses an element.

**The samples are a `source`, not a carrier.** `waveform({ data: sig })`, where
`sig = source(samples)` decides how they travel and stays addressable —
`sig.set(other)` rewrites the definitions holding it and pushes to every live
widget. The threshold is the Python client's, so a source of the same length
makes the same decision in both; **what differs is the spill**. A page has no
temp file, so a source past the ceiling rides a **blob** beside the JSON and its
index is assigned at `open` — which is `blob: 0` stopping being a
correspondence kept by hand. The blob has no live door in the host, so `set` on
a spilled source refuses rather than pretending; a native client rewrites its
file and re-reads it.

**A control widget is built from the control it drives.** `knob(freq)`,
`slider(sd.control("amp"))`, `number` and `toggle` read the control's **name**
and **default** off it, so the widget and the graph cannot disagree about what
`"freq"` is; the **range is the widget's** (`knob(freq, { min: 110.0, max:
880.0 })`), since a control is a signal in a graph and says nothing about how a
knob is drawn. Only a Faust parameter arrives with a range, from the `hslider`
that declared it. All three def families answer with the same `ControlInfo`
shape, spelled `sd.control("freq")` where Python writes `sd["freq"]`: a class
here cannot take a bracket without an index signature over everything else on
it. Then the whole surface binds in one verb — `win.bind(synth)`,
`win.unbind()`, `win.controlMap()` — and `win.widget("freq").bind(...)` is still
there for a bus, another widget or an arbitrary address.

**Nothing is pumped.** The page already has an event loop, so the Python
client's `pump` has no counterpart: a handler fires when the message arrives.
Building and opening are synchronous; anything that waits for the host to
*answer* is a promise:

```ts
const win = host.open(tree);                       // a WindowHandle, now
win.widget("cutoff").set({ value: 2000.0 });
win.widget("cutoff").onEvent((v) => console.log("cutoff ->", v));
const info = await win.widget("cutoff").query();   // a round trip, so awaited
```

Getting the host is the asynchronous part, since it loads wasm:
`const host = await GuiHost.page();`.

**Two ways to have a host, and one ambient rule.** `GuiHost.page()` is the wasm
host on this page's canvas — the ordinary one; `GuiHost.connect(url)` drives a
*native* `clausters-gui --ws` from the tab, which is the same object over a
different carrier and is this client's `attach`: it connects to a host it did
not open. Either becomes the **ambient** host if none is registered (first-wins,
the mirror of the audio server's default-session adoption), which is why
`view(...).open()` needs no argument, and `stop()` gives the registration up. A
`Session` opened with `Session.page()` carries one either way; a session drives
a host it did not open by being **given** one — `new Session(server, clock,
await GuiHost.connect(url))` — the way it is given a `Server`.
`newGuiHost()` boots an instance that is **not** the page's — its own engine
unless you hand it one — for a document holding several independent
instruments, and there you `attach(defId, canvas)` your own canvas.

**Bulk data is fetched, not mapped.** A `path` or a `cache` is a URL the host
fetches rather than a file it maps; a `buffer` is still pulled over the host's
client leg. Everything else about the source precedence is identical.

**The canvas is an element, and the document places it.** There is no window
manager: a GuiDef rooted in a `window` draws into a canvas that CSS sizes — one
per def, so several windows are several canvases. `open` makes each view its
own (in the element you name, else appended to the document) and hands it back
as `win.canvas`; closing the window releases it. Sizes stay what they always
were — logical pixels resolved through the page's `devicePixelRatio` — so a tree
written for the desktop comes up the same size in a tab. That substitution is
what makes a bundle mountable in the flow of a page: see
[Components](components.md).

A def fed straight through the binding surface, with nobody having said where,
draws on the page's **fallback** canvas — appended the first time that happens,
so a document whose views all name their own place never carries an empty one.

**The keyboard is shared with the page.** A canvas is focusable, and while it
holds the focus the host reads the keys: click a `text` field to type into it,
Tab to walk the window's focusable widgets. **Tab past the last one gives the
keyboard back to the document** — the canvas blurs and the browser's own tab
order carries on — so a GuiDef mounted in the flow of a page is never a
keyboard trap. A script points the focus itself with
`win.widget("name").focus()`, and hears every move as a `"focus"` event.

**The typeface is the page's to hand over.** The browser bundle carries the
host's glyph rasterizer but no face, so text draws with the embedded bitmap one
until the page fetches an outline font and passes the bytes:

```ts
const face = await fetch("/fonts/DejaVuSansMono.ttf").then((r) => r.arrayBuffer());
(await guiHost()).bridge.font(new Uint8Array(face));
```

It must be a raw **TrueType/OpenType** file (the rasterizer does not decompress
WOFF2, so a Google Fonts CSS URL is not one), served with CORS if it comes from
another origin — a CSS `@font-face` cannot serve here, since the host draws into
a canvas and never reads the document's fonts. Loading one relayouts nothing:
the sizing table never followed the typeface, so the same tree comes up the same
size before and after, and it may be handed over at any point. What changes is
that `textSize` is then continuous rather than quantized to half-steps of the
cell, which a bitmap glyph's own pixels require.

Composition (IME) and the system clipboard stay the **page's**: a canvas cannot
host an input method, so the host reads the keys it is handed and no more, and
the clipboard a field cuts and pastes through is its own, page-wide. Text that
needs composing is not entered through a host field today.

## Notation: engraving in the page

The `score` widget draws a page of music, and what it draws is a **display
list** — a glyph-outline table plus placed glyphs, staff lines, stems and beams
in page units — which the client produces and the host tessellates. The host
reads no notation and links no engraver; that split is what lets one host
renderer serve every client.

The engraving is `gui.notation`, and it is the same layer the Python client has:
the score model, the SVG walk and the MEI encoder are the shared Rust core, and
the engraver is [verovio](https://verovio.org) compiled to wasm from the same
pinned sources, with the same options, as the native client links. A page and a
window engrave one score into one drawing — a parity suite compares them
primitive by primitive.

```ts
import { gui, Event } from "clausters";
const { notation } = gui;

// The inverse direction, data → score: the client's own events become MEI.
const melody = [67, 69, 71, 72].map((midinote) => new Event({ midinote, dur: 1.0 }));
const score = await notation.Score.fromNotes(melody, { meter: "4/4", key: "G" });

const page = score.displayList();          // what is drawn, the cursors, the notes
const win = host.open(gui.window({ title: "score" },
    notation.scoreView(page, { name: "page", editable: true })));
```

Opening a score is the layer's one asynchronous step, and only because the
engraver is **fetched on demand**: nothing in the runtime imports it, so a page
that draws no notation never downloads it. Everything after that — drawing,
editing, undo — is direct.

An engraved page comes in three layers from one engraving, because the engraver
mints fresh MEI ids on every load and ids from two engravings do not line up:
what the host **draws**, where the **cursor** goes at each onset, and what
**sounds** (one `{t, dur, pitch, id}` per note, which stays in your script — it
is what a driver plays).

Editing rides on those ids. A click reports the element under the cursor; a
vertical drag reports the diatonic staff position the note *reaches* — absolute,
so an edit that arrives twice moves nothing the second time. Your script applies
it and sends the new page back:

```ts
win.widget("page").onEvent((tag, id, position) => {
    if (tag !== "transpose") return;
    score.transposeTo(id, Math.round(position));
    win.widget("page").set({ displayList: notation.pageJson(score.displayList()) });
});
```

`score.undo()` and `score.redo()` are the client's, not the host's: the score
owns a stack of MEI snapshots, and the host holds no score at all.

To **play** the page with the cursor following the sound, `gui.Transport` is the
same object every time view uses — a lane, a piano roll, an engraved page — and
`notation.transport` only fills in the page's unit, since a score places its
cursor in milliseconds where a lane places it in samples:

```ts
const tp = notation.transport(host, win.widget("page").id, {
    source: (at) => new seq.Playhead(timelineOf(score), clock, server).play({ at }),
    tempo: 2.0, sampleRate: engine.context.sampleRate,
    extent: () => endOfPiece(score),
});
tp.play(server);                 // and pause / stop / locate
setInterval(() => tp.update(), 100);   // parks the cursor when the pass ends
```

The line is the **host's**: `play` sends one anchor — the clock value the view's
time 0 maps to — and the host sweeps from it every frame, so a pass costs one
message and not one per frame. A pause writes the other half of that number, the
static cursor where the music stopped. `update` is the one thing a script owes
it: a pass ends when its last item *starts*, so the transport keeps sweeping
that last note's tail and parks only at the piece's extent.

The example is `examples/score.html`.

## Bindings, and the page that runs without a script

A widget's value can bypass this script entirely — to the audio server, or to
another widget:

```ts
win.widget("cutoff").bind("/node_set", 1000, "freq");
win.widget("picker").bindWidget(win.widget("pages"), "index");
```

Both matter more here than on the desktop, because a **bundle** is a GuiDef
whose widgets are bound and whose boot list starts its graph: the page mounts
it and no client library runs afterwards. Writing one is the Python client's
job ([bundles](https://clausters-python.readthedocs.io/en/latest/bundles.html));
mounting one is [Components](components.md).

## Reference

- Every builder, its options and its event payloads: the [API
  reference](api/Namespace.gui.md).
- The wire itself — the `/gui_*` commands, the axis properties, the edit-back
  payloads: the server guide's [GUI protocol
  chapter](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
- Reading buses and buffers *from the script*, to draw with your own code
  instead of a widget: [Reading the server](data.md).
