# The logical side: groups as signal graphs

Every group so far has been **concrete**: a relation in time between
contents placed in time. The other kind is **logical**: the members relate by
*processing* — a signal chain wired through buses — and time has nothing to do
with it. Same `Group`, different kind, and a different render: a logical
group does not flatten into a timeline, it translates into a `GraphDef` — the
server's own notion of a named configuration of nodes wired by buses.

## Two defs that wire

A member of a signal chain reads and writes buses through its own controls —
by convention named `in` and `out`:

```python
from clausters.defs import SynthDef, control, in_, out, sin_osc

freq, out_bus = control("freq", 440.0), control("out", 0.0)
tone = SynthDef("tone", out(out_bus, sin_osc(freq) * 0.15))

in_bus, level = control("in", 0.0), control("level", 0.4)
gain = SynthDef("gain", out(0.0, in_(in_bus) * level),
                        out(1.0, in_(in_bus) * level))

server.add_synthdef(tone)
server.add_synthdef(gain)
```

`tone` writes a sine onto whatever bus its `out` control names; `gain` reads
its `in` bus, scales it, and puts it on the hardware outputs. Neither def
knows the other exists — the wiring is the group's business.

## The logical group

```python
from clausters.form import Generator, Group

chain = Group(kind="logical", name="chain", buses=["mix"])
chain.add(Generator("tone", controls={"out": "mix", "freq": 220.0}))
chain.add(Generator("tone", controls={"out": "mix", "freq": 331.0}))
chain.add(Generator("gain", controls={"in": "mix"}))
```

The members are `Generator` elements — the *Function* kind, wrapping a def by
name. A control's value is a number (set at creation), the name of one of the
group's **buses** (`"mix"`, a private internal audio bus each instance
allocates for itself), or the reserved `"OUT"` (the hardware). Placement
offsets exist but are **ignored** here: a logical group is a signal graph, not
a timeline.

The translation is pure — inspect it before it goes anywhere:

```python
import json
print(json.dumps(chain.to_graphdef().spec(), indent=2))
```

Two tones summed on `mix`, one gain stage reading it: the 1:1 mapping of the
group onto a `GraphDef`. Render it — for a logical group that means
*send and instance*, not flatten and play:

```python
inst = chain.render(server)     # /d_graph, then /graph_new — it sounds now
```

A fifth and its gain stage, sounding continuously — a graph instance is a
running configuration, not a scheduled event. It lives until you free it:

```python
server.free(inst)                # the instance group and its private buses
```

One boundary, stated plainly: a logical group is rendered **on its own**. It
is not placed inside the concrete song and flattened with it — the two
kinds answer different questions (*what sounds when* versus *what is wired to
what*), and `render` routes each to its own path.

## The patcher

A logical group has a view too, and it is not a lane — its shape is not time.
Open an editor on the chain itself:

```python
patcher = Editor(chain, sample_rate=SR, tempo=TEMPO, title="chain")
pwin = patcher.open(gui)
```

A **patch**: the member boxes down one side (each with its wirable controls as
ports), the bus nodes down the other (`mix`, and `OUT` if a member names it),
and one wire per `(member, control) ↔ bus` connection.

Notice the patch is **undirected** — no arrows. That is deliberate honesty: a
`GraphDef` records that a control *names* a bus; which end writes and which
reads is the server's own analysis of the running graph, not something the
data knows. The view shows the connection and leaves the direction to the
engine, so it can never lie about signal flow.

## Rewire it

The wires are live, and the rhythm is the one you know:

- **drag a port onto a bus** — that control now names that bus;
- **drag a port onto empty space** — unwired.

Re-instance the chain so you can hear the difference, then unplug the gain
stage's input on screen (drag its `in` port to empty space), and:

```python
inst = chain.render(server)
```

```python
patcher.poll()                     # the edit lands on the group
print(chain.members[2][2].controls)   # {} — 'in' no longer names a bus
```

The edit rewrote the member `Generator`'s controls — the data again, nothing
else. And exactly as with a moved clip, what is *running* does not rewire
itself; the next render sends the graph as drawn:

```python
server.free(inst)
inst = chain.render(server)       # silent: the gain stage reads nothing
```

Wire `in` back to `mix` on screen, then:

```python
patcher.poll()
server.free(inst)
inst = chain.render(server)       # and it sounds again, wired as drawn
```

Clean up the demo:

```python
server.free(inst)
patcher.poll()                     # drain anything left
gui.close(pwin)
```

The piece itself never needed the logical side — but a real composition grows
one the moment two nodes share a bus: a send, a master chain, a layered
instrument. It is the same `Group`, the same five primitives, and the same
loop: build in code, see it drawn, edit either side, render.

Next: [Bouncing: the piece as a file](bounce.md).
