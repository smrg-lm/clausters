# The logical side: groups as signal graphs

Every aggregate so far has been **concrete**: a relation in time between
contents placed in time. The other kind is **logical**: the members relate by
*processing* — a signal chain wired through buses — and time has nothing to do
with it. Same `Aggregate`, different kind, and a different render: a logical
aggregate does not flatten into a timeline, it translates into a `GraphDef` —
the server's own notion of a named configuration of nodes wired by buses.

## Two defs that wire

A member of a signal chain reads and writes buses through its own controls —
by convention named `in` and `out`:

```python
from clausters.defs import SynthDef, control, in_, out, sine

freq, out_bus = control("freq", 440.0), control("out", 0.0)
tone = SynthDef("tone", out(out_bus, sine(freq) * 0.15))

in_bus, level = control("in", 0.0), control("level", 0.4)
gain = SynthDef("gain", out(0.0, in_(in_bus) * level),
                        out(1.0, in_(in_bus) * level))

tone.send(server)
gain.send(server)
```

`tone` writes a sine onto whatever bus its `out` control names; `gain` reads
its `in` bus, scales it, and puts it on the hardware outputs. Neither def
knows the other exists — the wiring is the aggregate's business.

## The logical aggregate

```python
from clausters.form import Aggregate, Generator

chain = Aggregate(kind="logical", name="chain", buses=["mix"])
chain.add(Generator("tone", controls={"out": "mix", "freq": 220.0}))
chain.add(Generator("tone", controls={"out": "mix", "freq": 331.0}))
chain.add(Generator("gain", controls={"in": "mix"}))
```

The members are `Generator` elements — the *Function* kind, wrapping a def by
name. A control's value is a number (set at creation), the name of one of the
aggregate's **buses** (`"mix"`, a private internal audio bus each instance
allocates for itself), or the reserved `"OUT"` (the hardware). Placement
offsets exist but are **ignored** here: a logical aggregate is a signal graph,
not a timeline.

The translation is pure — inspect it before it goes anywhere:

```python
import json
print(json.dumps(chain.to_graphdef().spec(), indent=2))
```

Two tones summed on `mix`, one gain stage reading it: the 1:1 mapping of the
aggregate onto a `GraphDef`. Render it — for a logical aggregate that means
*send and instance*, not flatten and play:

```python
inst = chain.render(server)     # /def_send graph, then /graph_new — it sounds now
```

A fifth and its gain stage, sounding continuously — a graph instance is a
running configuration, not a scheduled event. It lives until you free it:

```python
inst.free()                # the instance group and its private buses
```

One boundary, stated plainly: a logical aggregate is rendered **on its own**. It
is not placed inside the concrete song and flattened with it — the two
kinds answer different questions (*what sounds when* versus *what is wired to
what*), and `render` routes each to its own path.

## The view it has

A logical aggregate has a view, and it is not a lane — its shape is not time. It
is a **patch**: a box per member, drawn directed and typed (inlets on the box's
top edge, outlets on the bottom, each a wirable control of the def) with a cord
per `outlet -> inlet` connection. The buses are not drawn: a cord *is* a bus.

The direction is not guessed — it is structural, read from each def (a control
feeding an `In` is an inlet, one feeding an `Out` an outlet), so the picture
reads as signal flow, top to bottom, and cannot lie about it. The `patch` widget
and how a script builds one are in
[The visual elements](../gui.md); what belongs on this page is the structure it
draws, which is the aggregate above.

The piece itself never needed the logical side — but a real composition grows
one the moment two nodes share a bus: a send, a master chain, a layered
instrument. It is the same `Aggregate` and the same five primitives.

Next: [Bouncing: the piece as a file](bounce.md).
