# The visual elements in a page

The GUI host is the same host the desktop runs, compiled to wasm and drawing on
a `<canvas>` in your document — so the widget tree, the props, the events and
the bindings are the ones the [Python client's GUI
chapter](https://clausters-python.readthedocs.io/en/latest/gui.html) walks
through, and that page is the tutorial. This one says what the browser makes
different, and nothing that book already says.

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

gui.window(
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

**Two ways to have a host.** `GuiHost.page()` is the wasm host on this page's
canvas — the ordinary one; `GuiHost.connect(url)` drives a *native*
`clausters-gui --ws` from the tab, which is the same object over a different
carrier. A `Session` opened with `Session.page()` carries one either way.
`newGuiHost()` boots an instance that is **not** the page's — its own engine
unless you hand it one — for a document holding several independent
instruments, and there you `attach(defId, canvas)` your own canvas.

**Bulk data is fetched, not mapped.** A `path` or a `cache` is a URL the host
fetches rather than a file it maps; a `buffer` is still pulled over the host's
client leg. Everything else about the source precedence is identical.

**The canvas is an element, and the document places it.** There is no window
manager: a GuiDef rooted in a `window` draws into a canvas that CSS sizes,
appended for you by default or handed over with `attach`. Sizes stay what they
always were — logical pixels resolved through the page's `devicePixelRatio` —
so a tree written for the desktop comes up the same size in a tab. That
substitution is what makes a bundle mountable in the flow of a page: see
[Components](components.md).

**The keyboard is shared with the page.** A canvas is focusable, and while it
holds the focus the host reads the keys: click a `text` field to type into it,
Tab to walk the window's focusable widgets. **Tab past the last one gives the
keyboard back to the document** — the canvas blurs and the browser's own tab
order carries on — so a GuiDef mounted in the flow of a page is never a
keyboard trap. A script points the focus itself with
`win.widget("name").focus()`, and hears every move as a `"focus"` event.

Composition (IME) and the system clipboard stay the **page's**: a canvas cannot
host an input method, so the host reads the keys it is handed and no more, and
the clipboard a field cuts and pastes through is its own, page-wide. Text that
needs composing is not entered through a host field today.

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
