# Auto-sorted groups (`/group_sortMode`, `/group_queryTree`, `/group_dumpGraph`)

In scsynth, execution order is the client's problem: a node reading an audio bus must be placed *after* the nodes writing it, by hand, with add actions and `/node_before`/`/node_after` — get it wrong and you hear silence or a one-block delay. Clausters can instead infer the dependency graph **from the bus connections themselves** and keep a group sorted automatically, so groups behave like the channels of a multitrack editor: drop nodes in, the server wires the order.

## Commands

```text
/group_sortMode  groupID mode      # 1 = auto-sort, 0 = back to manual
/group_queryTree [groupID] [detail]  →  /group_queryTree.reply  (scsynth format at detail 0/1)
/group_dumpGraph [groupID]         →  /group_dumpGraph.reply  (debug string)
```

- **`/group_sortMode groupID 1`** sorts the group's children right away and re-sorts on every change that can affect the order: a node added or removed, a def's bus retargeted via `/node_set`, a subtree moved in. The root group (0) is allowed. Inside an auto group, manual `/node_before`/`/node_after` reply `/fail` — ordering is the group's job now; `mode 0` hands it back. All of it is schedulable in timed bundles and usable in NRT scores.
- **`/group_queryTree [groupID = 0] [detail = 0]`** replies with the scsynth-style tree listing: `detail`, the group, its child count, then depth-first per node the ID and child count (`-1` marks a synth), the synth's def name, and — from `detail = 1` — control count and (name, value) pairs (`detail = 2` appends the full node record; see [`schemas.md`](schemas.md)).
- **`/group_dumpGraph [groupID = 0]`** replies with a human-readable view of what the analysis inferred: per child, the buses read and written and whether it is `dynamic` (see below).

## How the analysis works

Per def, the server records which audio buses each node **reads** (`In`, a Faust def's `in..in+inputs`) and **writes** (`Out`, `ReplaceOut`, a Faust def's `out..out+outputs`):

- A bus index that is a **constant or a control** is static. Controls use the node's current value — an `/node_set` on a control used as a bus index re-analyzes and re-sorts on the spot.
- A bus index computed by a **signal** (a UGen wire) cannot be analyzed: the node is marked **dynamic** and acts as a conservative barrier — it keeps its position and nothing is sorted across it.

A node that writes a bus another reads must run first; that is the whole DAG. `ReplaceOut` counts as read+write (it consumes what is on the bus), so an insert fx (`In 16 … ReplaceOut 16`) lands after the sources summing into 16 and before the readers. Pure summing writers to the same bus get no edge between each other — mixing commutes.

Two things the sort deliberately does **not** do:

- **Cycles are not "solved".** Legitimate feedback (read-before-write across two nodes) keeps the nodes' current relative order, which means one block (64 samples) of feedback delay — exactly like a return send in a multitrack editor. Chained insert fx on one bus are a cycle by this definition too, and they correctly keep their insertion order.
- **Nothing crosses a dynamic barrier**, even when the static part of the graph would want to. If you need dynamic bus routing *and* auto-sorting, isolate the dynamic node in its own (manual) group.

Sorting is per group, treating each child as a unit: a child *group* counts as the union of its whole subtree's reads/writes. Nested auto groups sort themselves independently.

## Mechanics and caveats

Everything runs on the network thread against a mirror of the node tree; re-sorts reach the audio thread as ordinary node moves, so the engine (and its real-time guarantees) is untouched. The mirror reflects commands **as sent**: messages inside a not-yet-fired timed bundle are mirrored immediately, so `/group_queryTree` can briefly show a future state, and a re-sort racing a scheduled bundle converges at the next change. Order inside an auto group is a *consequence* of the analysis — clients should stop carrying add-action logic for ordering and just use `head`/`tail`.

`python3 examples/auto_order.py` demonstrates the whole flow: a source → fx → master chain built deliberately backwards, silent in a manual group, audible the moment `/group_sortMode 1` arrives, with the inferred graph printed before and after.
