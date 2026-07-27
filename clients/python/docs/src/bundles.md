# Bundles: an instrument you can hand to a page

Everything else in this guide runs an instrument *from a script*: the client
builds defs, sends them, plays them, and stays in the loop. A **bundle** is the
other posture — the instrument written to a directory, and the script gone.

That directory is what a browser tab mounts as an HTML element, what
`clausters-gui --standalone` opens on the desktop, and what a loopback host
plays against a running server. Same files, three legs. Nothing of the Python
client runs at mount time; it only writes.

```python
from clausters.bundle import Bundle

b = Bundle("fm-voice")
b.param("freq", float, default=220.0, min=60.0, max=700.0)

lfo   = b.bus("lfo")        # -> "@lfo"
graph = b.node("graph")     # -> "@graph"

b.synthdef(voice())         # named "fm-voice.voice"
b.graphdef(rig())           # named "fm-voice.graph"
b.gui(scene(lfo, graph))
b.boot(["/graph_new", "fm-voice.graph", graph, 0, 0, "lfo_bus", lfo])
b.preset("bright", freq=660.0)
b.write("./fm-voice")
```

and the page that plays it, in full:

```html
<script type="module">import "./fm-voice/index.js";</script>
<fm-voice></fm-voice>
<fm-voice freq="110"></fm-voice>
```

## Why the symbols

Those two elements are the same bundle mounted twice. They cannot share a node
id or a bus — one would silence or overwrite the other — so whatever the
instrument *allocates* is named rather than numbered. `bus`, `node` and
`buffer` each declare a symbol and hand back a placeholder string, which reads
naturally wherever an index goes:

```python
meter(4, lfo, label="lfo")               # the widget watches "@lfo"
b.boot(["/n_set", graph, "rate", 4.0])   # the message names "@graph"
```

When a page mounts the bundle it allocates one id per symbol and fills the
placeholders in. The two instances get different ones, and neither knows it.

`param` is the other kind of hole: a value the *markup* supplies, as an
attribute on the tag, resolved **attribute → preset → declared default** and
type-checked on the way in. A parameter with no default is required, so
mounting without it is an error rather than a silent zero.

## The one rule

Placeholders live in the GuiDef and in the boot list — never in a def payload.
That is what makes a second instance cheap: the def payloads are identical
between instances, so they are sent to the server once and shared. It leaves
one rule for the author:

> A bus, a node or a buffer reaches a def **as a control**, never as a baked
> constant.

```python
# no: the bus number is compiled into the def, so both instances write bus 0
return SynthDef("voice", out(0.0, sig), out_ctl(0.0, env))

# yes: the def takes the bus it is given, and the mount gives each its own
return SynthDef("voice", out(0.0, sig), out_ctl(control("env_bus"), env))
```

`write` checks this through the shared core before emitting anything, along
with every other way a bundle could fail to mount — an unknown symbol, a
default of the wrong type, a widget id used twice. **An unmountable bundle is
unwritable**, which is the whole reason to validate at write time: the error
belongs to the author, not to the reader of a page.

## Names

`Bundle("fm-voice")` names three things at once. It **prefixes the def names**
(`voice` becomes `fm-voice.voice`), because a def name is a global namespace on
the server and two bundles defining `voice` differently must not collide. It
is the GuiDef's name, which `--standalone` takes. And it is the custom
element's tag — HTML wants a hyphen in one, so a one-word name (perfectly good
on the desktop) needs an explicit `write(tag="…")`.

Widget ids are **local**: the root is 1 and the rest are yours to number from
2 up. The mount offsets the whole block per instance, so the numbers never
travel.

## Running what you wrote

```sh
# in a tab: serve the package root, and open the page that imports index.js
cd clients/web && ./build.sh && python3 -m http.server

# on the desktop, self-contained, from clients/gui
cargo run --features standalone --bin clausters-gui -- \
    --standalone fm-voice --data-dir <dir>
```

Worked examples: `clients/web/examples/piano/make_bundle.py` (a keyboard whose
keys the host itself turns into voices) and
`clients/web/examples/graph-controls/make_bundle.py` (a GraphDef's control
surface), with `clients/web/examples/document/` showing both mounted in one
interactive text. The format itself — the manifest, the two sigils, the
two-phase mount — is documented in the server guide's
[clients chapter](https://clausters.readthedocs.io/en/latest/clients.html).
