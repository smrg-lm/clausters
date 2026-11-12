# Plan — High-level clients for Clausters, with a shared native Rust core

This plan covers the **Python** client, the reference client. It was written to serve any other language as well, and the JavaScript/TypeScript client — the **web** client, roadmap in `clients/web/PLAN.md` — is built on the shared design recorded here: the same native Rust core and the same C-ABI contract (as wasm in the browser). The language-specific part is only the coroutine driver and the thin binding wrappers.

> **Note — sc3 as the reference model.** For any design or semantics question (module structure, clock/routine behavior, events, patterns, OSC/MIDI interfaces, names, conventions), fall back to [sc3](https://github.com/smrg-lm/sc3) as the model. This client is a clean, pruned rewrite, but sc3 is the source of truth on how these pieces should combine and behave; deviate from it only with an explicit reason (the Clausters-specific parts: FaustDefs, server resources, native Rust core).

## Build strategy — finish one client, then port (keep the seam modular)

"Build all clients at once vs. finish one then port" is a false binary: what makes a port cheap is **where the logic lives**, not the order. The rule for this project, across all clients (Python the reference, the web client in TypeScript, any future language/platform):

- **One reference client at a time.** Finish and polish the Python client first — it is the most mature. Do not grow two full clients in parallel: that duplicates tests and bugs and lets them diverge.
- **Push every language-agnostic piece down into the shared core as you write it** (not "later"). If you find yourself writing protocol/value/time logic inside Python, it belongs in `clausters-core`/`clausters-ffi`. This is what makes the later port mechanical rather than a rewrite. The GUI track proved it: carving the platform seam first (the `host` traits) made the browser port reuse the native code verbatim.
- **Porting reuses the core, never reimplements it.** A new client = rebind the same core (ctypes/N-API natively, wasm in the browser, mirroring `crates/clausters-gui`'s wasm path) + add only the parts that genuinely cannot be shared.
- **What is never shared (and should not be forced into the core):** the idiomatic/ergonomic API surface and the concurrency/scheduling model (Python threads + TempoClock vs. JS event loop/async). The core provides the time/value primitives; each language writes its own coroutine driver and ergonomics. See "Guiding principle of the seam" below.

Net: each milestone is built and finished on the reference client, but always factored so that only the thin language-specific shell remains to write per platform.

## `FormEditor` was removed (2026-09-06)

The multitrack driver written over `clausters.form` is gone, with its examples,
its tests and the book chapter built on it. Every `FormEditor` mention below
sits inside a **closed** milestone and is left as the record of what shipped;
nothing there is a pointer to code that exists.

`clausters.form` itself is retained, frozen: a small set of client-side data
structures for placing elements in time, with no view, taking no new work.

**`Editor` and `Editing` are untouched and stay**, along with `View`, `Domain`,
`Echo`, `edit(x)` and the three editors it opens (`SamplesEditor`,
`PointsEditor`, `NotesEditor`). What was removed is the *arrangement* driver
above them, not the seam. **They now have other priorities to adapt to**: the
three classic applications are built over the session in `clausters-document`,
so what `Editor` has to serve is an application that holds a document rather than
a client tree — `O23` (the host reconciles rather than frees) and `O24` (the
three applications) are where that adaptation is decided, and it should not be
guessed at from this side first.

The reason is structural rather than a defect count. A multitrack's own state -
which track a thing is on, its order within the track, its placement and its
identity - is authored, durable and undoable; projected out of a general tree it
had nowhere to live but the widget tree, which is drawn, and drawing frees. It
is being rebuilt as a **session** in `crates/clausters-document` - source,
region, lane, track, automation - with three classic applications over it
(audio editor, multitrack editor, score editor). See that crate's `PLAN.md`,
"The turn: the arrangement stops being a projection" (`O21`-`O24`).

**The seam under them was reworked after, on the application-scope track**
(`crates/clausters-gui/PLAN.md`, AP track): an `Application` owns a window set, a widget id
is named after what it draws rather than leased, the host reconciles a def so no
client keeps a picture, and the undo order is the applications crate's editing
context, which `Editing` binds. The reasons are in `docs/decisions.md`.

## Context

Clausters is the Rust audio server (scsynth-style) controlled over OSC. Today the only client in the repo is `clients/python/clausters.py`: the **low-level transport layer** (embed cdylib / shm / render), stdlib-only, with the boundary rule "only flat data crosses" (bytes in, `array('f')`/floats/ints out). There is no high-level layer: building defs, resources, events and sequencing is currently left to the user.

The goal is a **high-level client** that selectively ports the core features of [sc3](https://github.com/smrg-lm/sc3) (a SuperCollider port to Python), but **centered on FaustDefs** instead of SynthDefs, and reusing the server's resources (buses, buffers, generator units). In parallel a **native Rust core** is extracted (TempoClock, numeric builtins, OSC assembly) shared by the server and by every client (Python and the web client today, any language later), so that client-side operations are **numerically equivalent** to the server's by construction wherever possible.

Agreed decisions:
- **Repo**: clean rewrite in `clients/python/` (sc3 as reference, without dragging in SynthDef or the full class library).
- **Rust**: turn `clausters` into a Cargo **workspace** and extract a core crate (`clausters-core`).
- **Binding**: a single **C-ABI** over the core, with thin per-language wrappers (ctypes/cffi in Python; wasm in the web client, which took that door over N-API).
- **Seam**: the Rust core owns builtins, TempoClock (queue + arithmetic) **and** OSC bundle/timetag assembly + conversion against the sample-clock; boundary "only flat data, no callbacks". The **coroutine driver (`yield`) stays in each language** — control flow does not move into Rust.

## Guiding principle of the seam

What is value or time transformation (language-agnostic) lives in Rust; what is the language's control flow (the `Routine`s that `yield` in Python, generators/async in JS, the pattern ergonomics) lives in each language. The loop that resumes the `yield`s is handled by the language: it asks the Rust queue "what's next and when?", sleeps, resumes the routine and returns the next time to Rust. There are no Rust callbacks into the host language — that preserves multi-language portability and the "only flat data crosses" rule.

## Target architecture

### Rust workspace (repo root)

Turn the current single crate into a workspace. Proposed layout:

```
Cargo.toml                  # [workspace] members
crates/
  clausters/                # current server crate (bin + lib + features realtime/faust/embed)
  clausters-core/           # NEW: pure kernels, no I/O, no alloc on the hot path
  clausters-ffi/            # NEW: C-ABI cdylib over clausters-core (the "lib for all clients")
clients/
  python/                   # high-level Python client
    PLAN.md                 # this plan (its design is shared by every client)
```

`clausters-core` (pure library, a `no_std` candidate except where it needs `alloc`):
- **builtins**: unary/binary ops over scalars and over `&[f32]` slices — the same formulas as the server. Base set: `add/sub/mul/div` (already native in the server), and the higher math that today exists in the server only via Faust (`sin/cos/tan/exp/log/sqrt/abs/floor/ceil/min/max/pow/atan2/...`, see `crates/clausters/src/faust/signals.rs`).
- **tempoclock**: time-priority queue + beat↔second↔sample arithmetic, tempo/meter, conversion against the server's sample-clock (read via `/clock_query` or via the shm data-plane).
- **rng**: a seeded generator that **replicates** the server's (`WhiteNoise` uses splitmix64/xorshift, `crates/clausters/src/dsp/noise.rs`) for client/server reproducibility.
- **osc**: message/bundle assembly with NTP timetag, reusing `rosc` (already a server dependency). `timetag ↔ sample target` conversion for `/sched_at` and bundles.

`clausters-ffi`: a cdylib that exports the core's C-ABI (explicit ABI version, like the current embed in `crates/clausters/src/embed.rs`). Distinct from the embed's `libclausters.so` (that is the in-process server; this is the client core). Two separate cdylibs, both consumable by ctypes/N-API/wasm.

### Numeric equivalence — a realistic contract

- Ops the server computes **natively** (`add/sub/mul/div`, `Sine` phase, `WhiteNoise` RNG): refactor the server to use `clausters-core` → **bit-exact by construction** (single source of truth). Mind RT-safety: `#[inline]` functions, no alloc/lock/IO (CLAUDE.md, `crates/clausters-server/tests/rt_safety.rs`).
- Higher math that in the server exists **only via Faust/LLVM** (`sin`, `log`, etc.): `clausters-core` implements the **same formula/semantics** (libm), but bit-for-bit equality with Faust's LLVM codegen is **not guaranteed**. Contract: same formula + documented tolerance; parity tests with tolerance.

### Client package (Python example; the web client mirrors the same structure)

Clean rewrite, mirroring sc3's structure but pruned, and carrying both def
families (SynthDef and FaustDef) as peers:

```
clients/python/
  pyproject.toml
  clausters/                       # high-level package
    __init__.py
    base/                          # selective port of sc3/base
      absobject.py  builtins.py  stream.py  clock.py  main.py
      netaddr.py    _oscinterface.py  _midiinterface.py
    seq/                           # port of sc3/seq: event, pattern, streampatterns
    defs/                          # port of sc3/synth, pruned to Clausters (Faust now, SynthDef later)
      faustdef.py  signals.py  (synthdef.py + ugens.py later)
      node.py  bus.py  buffer.py  server.py
    _native.py                     # ctypes wrapper over clausters-ffi (Rust core)
    transport.py                   # = the current clausters.py (embed/shm/render), relocated
```

- `transport.py`: the current `clients/python/clausters.py` is kept as the transport layer (do not rewrite; it is orthogonal to the core). The high-level package leans on it to talk to the server.
- `_native.py`: ctypes over `clausters-ffi` (builtins, TempoClock, OSC assembly). Flat-data boundary, like `transport.py`.
- `base/builtins.py` + `base/absobject.py`: `AbstractObject`/operands dispatch the ops over a scalar **or list** to `_native` (equivalence with the server). Where per-scalar FFI overhead does not pay off, a pure-language fallback identical in formula.
- `base/clock.py`: `TempoClock` wraps the native queue+arithmetic; the scheduling loop (resuming `yield`) stays in the language.
- `base/stream.py` / `seq/`: coroutines with `yield`, patterns and events — pure Python (in the web client: generators/async).
- `defs/signals.py`: **the user interface for building FaustDefs**. It provides a library of **lowercase callables** (functions or callable objects) that map, in principle, the **Faust Signal API** (`sin`, `cos`, `add`, `mul`, `delay`, `select2`, `hslider`, `rdtable`, …). The **composition** of these callables is what builds the graph: a specification serialized to a **JSON signal tree** now (and a **box tree** later) to send with `/def_send faust` (see `crates/clausters/src/faust/`). Firm design convention: **lowercase names even for objects that act as functions** — a quality that eases programming work in Python (fluent expression-style composition). The **same pattern is reused for UGens** (`ugens.py`, constructors of the SynthDef graph in JSON).
- `defs/faustdef.py`: **the client's center**. It takes the graph built with `signals.py` (or direct Faust source) and produces the def for `/def_send faust` in its three forms (source, JSON box tree, JSON signal tree); it manages controls (UI labels → control names; reserved `out`/`in`). On-disk persistence/cache is handled by the server (M16, bitcode cache). `synthdef.py` (later) does the analogous thing for the UGen graph.
- `defs/{node,bus,buffer,server}.py`: client-side ID allocators (scsynth-style: nodes, audio buses 0..127 / control 0..1023, buffers 0..1023), handling of `/done`/`/fail`, `/server_notify` → `/node_start`/`/node_end`. NRT: score → transport's `render()`.
- **To port later from `sc3/synth`** (remember): `synthdef.py` + `ugen.py` (client representation of the UGen SynthDefs), `synthdesc.py` + `spec.py` (control specs; in Clausters only `InCtl` exists as a control UGen), `_graphparam.py` (adapts Python types to the types the nodes receive; reviewable, need not be identical, deferrable).

### Separation of responsibilities: server-agnostic client vs server representation

There are **two groups of abstractions** that must stay well separated (see memory `separacion-cliente-servidor-clausters`):

1. **Server-agnostic** (knows nothing about transport or the server app): **timing** (`base/clock.TempoClock`), **sequencing** (`base/stream`, `seq`) and **JSON graph generation** (`defs/signals`, `defs/faustdef`, `base/absobject`, `base/builtins`).
2. **Representation + configuration of the Clausters server**: the **`Server`** class (`defs/server`) = the running server; the **resource handles+allocators** (`defs/node`, `defs/bus`, `defs/buffer`); and the **communication interface** the `Server` owns. Choosing communication over **shared memory** or **embed** = adding a **new communication interface** to the `Server`.

Correspondence with the Clausters server:

| Python (client) | Represents | Counterpart in the server |
|---|---|---|
| `defs/server.Server` | the running server + its communication | the `clausters` process (OSC/UDP; later shm/embed) |
| `defs/node` (`Synth`/`Group`) | handles + id allocator | `crates/clausters-server/src/node` (node tree) |
| `defs/bus` (`Bus`) | audio/control buses + allocator | buses in `crates/clausters-server/src/dsp` |
| `defs/buffer` (`Buffer`) | buffers + allocator | `src/dsp/buffer` |
| `base/clock`, `base/stream`, `seq` | timing and sequencing | — (agnostic) |
| `defs/signals`, `defs/faustdef` | client-side JSON graph | `/def_send faust`, `crates/clausters-server/src/faust` |

### Target interfaces and time handling (RT / NRT / MIDI) — the central piece

The point that makes **one and the same clock-and-routine logic** serve real time, deferred render and MIDI without rewriting it. The correct split is:

- The **clock** (`base/clock.TempoClock`) only schedules and provides time (beat↔second↔sample math via `_native`, scheduling queue, RT/NRT drives, resuming `yield`). **It does not communicate with the server.**
- The **`Server`** owns the **target/communication interface** and **emits** the events, computing the timetag from the logical time of the running routine's clock (`main.current_tt`). Changing the interface changes *where* and *in which mode* (live vs deferred) the events go; clock and routines do not change.
- `base/_oscinterface.py`: `OscUDPInterface`/`OscTCPInterface` (RT; TCP not in the server yet) and `OscNrtInterface` (accumulates into `OscScore` → `render()`). `base/_midiinterface.py`: `MidiRtInterface`/`MidiNrtInterface`+`MidiScore`. shm/embed would be additional communication interfaces of the `Server`.

> **Post-C3 correction:** in C2 the communication ended up **misplaced in `TempoClock`** (fields `target`/`interface`, methods `send_bundle`/`send_msg`/`_emit`/`_when`). Milestone **C4** moves it to `Server`. The clock keeps only timing.

## Closed tracks

The tracks whose every milestone shipped are in [`docs/history/plans/python.md`](../../docs/history/plans/python.md),
moved there whole; the plan keeps what is open. The ticked entries of "Found by
use" and "Future directions" are at the end of the same file. The tracks are,
by their headings:

- Milestones (client "C" track, parallel to the server "M" track)
- Future milestones (client "C" track, parallel to the "M" track)
- The arrangement model + the multitrack editor (client arc, phased)
- The generic editor: one editor per domain, and `edit(x)` (client arc, phased)
- A sequence's structures are objects (client arc, phased)
- A script plays and holds what the editors do (client arc, phased)
- The numeric builtins: SuperCollider's vocabulary (client arc, phased)
- Patterns: SuperCollider's operators and library (client arc, phased)
- The notebook client (`clausters-jupyter`) — moved to the `jupyter` branch

## The client API reform: the GUI element as an object (client arc, phased)

A reform of the client's GUI surface, in two parts: **first the shape of the
objects** (a builder returns a thing, not an anonymous `dict`), **then their
interaction with the audio server** (a widget takes the control it drives).
Both are done **with the host and the wire as they are** — every milestone here
is client-side. Where one of them wants something the host or the server does
not offer, that is named as a limit and left to the GUI/server tracks rather
than smuggled in.

The premise, in one line: a `SynthDef` is an object that builds a JSON AST and
knows how to send itself; `guidef.window()` returns a bare `dict` and knows
nothing, which is why the host had to become the subject of the sentence
(`host.open(tree)`) while every other resource is its own subject
(`sd.send(server)`, `plot(obj, host=None)`). Giving the GUI node the same shape
the def already has removes the asymmetry rather than papering over it.

#### Part 1 — the objects

- ✅ **C39 — `View`: a GUI node is an object that carries a tree** *(done
  2026-08-23)*. Every builder in `clausters.gui.guidef` returns a `View`
  (`clausters.gui.view`) instead of a `dict`. `View` **is a `dict` subclass**, so
  the JSON it produces is byte-identical to today's and neither the wire, the
  host, nor `to_json` changes; what it adds is behaviour: the name index
  (`find`/`names`), `to_json()` and `open()`, with composition by nesting
  unchanged at the call site. The UGen analogy is the one to keep: a `View` is a
  node of an AST that a program builds and then sends, not a live widget — the
  live widget is what `open()` returns.

  ```python
  # The builders compose exactly as they do today; the value is now a View.
  v = window(
      layout(
          knob(name="freq", label="freq", min=110.0, max=880.0, value=220.0),
          slider(name="amp", label="amp", min=0.0, max=1.0, value=0.2),
          flow="col"),
      title="voice")

  v.to_json()      # the same document /gui_def already takes
  w = v.open()     # a live window, on the ambient host
  w["freq"].set(value=440.0)
  ```

  **The bracket is the dict key, not the name.** The plan first asked for
  `View.__getitem__(name)`; that collides head-on with the thing `View` *is* —
  `node["type"]`, `child["id"]` and `node.get("children")` are read all over the
  client and by the host's own id walk, so one bracket cannot mean both. The
  document is addressed by key (`v["min"]`) and the tree by name (`v.find("freq")`,
  `v.names()`). On the **live** side the bracket stays the name (`w["freq"]`),
  where there is no document to collide with — a `WindowHandle` indexes nothing
  else.

  **The name is a client-side index, and it must stop being an id.** The host
  does not read a widget's `name` at all (`to_json` strips it before the document
  goes out; only the *root*'s survives, and there it means "persist this def"),
  so `w["freq"]` is a table the client builds by walking the tree. Two
  consequences landed with it: a **duplicate name in one scope is an error**,
  raised both where the `View` is built and where `GuiHost._register` walks a
  hand-written tree — not the silent last-wins of before, which left the shadowed
  widget drawing and unreachable (the project's recorded stance on silent
  shadowing is `crates/clausters-server/src/osc/server/dispatch.rs`); and a **nested `View` scopes its
  names**, so two sub-views can both hold a `freq` and `v.find("osc1").find("freq")`
  reaches one of them. (Open: whether a flat `"osc1/freq"` path is offered as
  well, and how a `WindowHandle` indexes a nested view — the handle's map is
  still flat, which no tree exercises yet because nothing nests a `window`. It
  will once `view()` lands in C41.)

  **The ambient host, made symmetric with the ambient server.** `open()` with no
  `host=` resolves the way `plot`/`scope` already did, and two adoptions were
  added so that resolution actually finds the host a script booted:
  `GuiHost.boot(adopt_ambient=True)` registers itself when none is registered —
  the mirror of `Server.boot`'s `adopt_default`, first-wins, spelled the same way
  — and `stop()` gives the registration up. `Session.gui()` inherits it through
  the same call. The ambient layer's own owned host (`plot._ambient_host`) boots
  with `adopt_ambient=False`, because it is the fallback and must stay
  replaceable (`scope` reboots it wired to a server).

  Docs: the client book's GUI page opens with the view-opens-itself form and the
  sessions page states the adoption rule; `clausters.gui.view` added to the API
  reference. Examples: `window.py` and `bind.py` (the two smallest, one
  per door — a free-standing `GuiHost().boot()` and a `session.gui()`).
  Tests: `tests/test_view.py`, plus the duplicate-name refusal in
  `tests/test_gui_host.py`. **Not ported to TypeScript yet** — the shape is
  written into `clients/web/PLAN.md`.

- ✅ **C40 — Ids belong to the instance, not to the tree** *(done 2026-08-23)*.
  `GuiHost._register` stamped `child["id"]` **into the caller's dict** and reused
  whatever it found (`if "id" not in child`), so a `View` opened twice handed the
  second instance the first one's widget ids; the host answers a colliding id by
  **skipping the subtree with a warning** (`registry.rs`, `"widget id {id}
  already in use, skipping"`), so the second window drew wrong instead of
  failing. This was the one defect blocking everything above: a def that cannot
  be instanced twice is not a def.

  **Ids were already automatic — what was wrong is where they were written.**
  Nobody spells an id: `alloc_id` draws them from the client's pool and the walk
  hands them out. Stamping them *into the caller's tree* is what turned a
  definition into an instance. And the host cannot take the job over, not even
  with a change: `/gui_def <id> …` takes the id as its argument and `/gui_set`,
  `/gui_free` and `/gui_bind` all address by it, so a host-assigned id would have
  to be reported back and every command would start waiting for a reply. The
  client-side pool is what makes the whole surface fire-and-forget — the same
  reason the audio server does not assign node ids either.

  **The walk copies.** `_register` became `_stamp`, which returns a *copy* of the
  tree with the ids filled in; that copy is what is serialized, and the caller's
  tree is never written into. The plan first asked for an id map in the handle
  keyed by path — that turned out to be bookkeeping for something the copy gives
  for free: every visit builds a fresh node and asks for a fresh id, so node
  identity never enters, and the **same `View` nested twice in one tree** gets two
  id runs exactly as two `open()`s do. The handle keeps only what it already
  kept, `name -> id`.

  ```python
  strip = window(knob(name="gain"), toggle(name="mute"))

  a, b = strip.open(), strip.open()     # two windows, two id runs, one view
  a["gain"].set(value=0.5)              # b is not touched
  ```

  The `View` stays free of ids; an explicit `node(id=…)` is still honoured and is
  then the caller's problem, exactly as an explicit node id is on the server — a
  hand-picked id on a subtree used twice **is** used twice, and the host skips the
  second. The duplicate-name rule from C39 already forces a repeated *named*
  sub-view to be named apart or not named at all, so the handle's name index
  stays unambiguous under this.

  Docs: the `guidef` header and `define`/`open` say "in the document it sends";
  the client book's GUI page gains the two-windows-from-one-view block and the
  sessions page stops telling the reader to read an assigned id back out of the
  tree. Example: `panel.py` opens its panel twice and drives both by the same
  names. Tests: ids read out of the sent JSON, one tree opened twice, and the
  same subtree nested twice (`tests/test_gui_host.py`), plus the two
  `tests/test_gui_ids.py` cases that read ids off the tree.

- ✅ **C46 — The source: the data a view draws is an object, not an index**
  *(done 2026-08-23; numbered past the track's last label so the numbers already
  written here keep their places)*. `signal`'s documentation already named the
  thing — **the source** — and already listed its five carriers for addressable
  samples: `data` (inline JSON), `blob` (an index into the message's trailing OSC
  blobs), `buffer` (a server buffer number), `path` (a mapped file), `cache` (a
  peak pyramid). Choosing among them was the caller's, and `blob=0` was a
  correspondence kept by hand between two places in the program.

  ```python
  sig = source(decaying_sine(8_000, 120.0))

  v = view(label(name="caption", text="..."),
           waveform(name="wave", data=sig),
           title="...", w=720, h=360, flow="col")

  win = v.open()             # no positional blobs: they come out of the tree
  sig.set(other_samples)     # the definition and every open view follow
  ```

  What it settles:

  - **The object picks the carrier**, not the person writing the view: short
    stays inline, long spills to a temp raw-f32 file the host maps. The
    threshold and the spill are `guidef.INLINE_MAX` / `guidef.spill`, which
    `clausters.plot` now takes too — "how large data reaches the host" decided
    once rather than in two places.
  - **One source in two views is one payload and two references** — what
    "interchangeable" meant, said by the program rather than by convention.
  - **`set` is the update door.** A source records the definitions it feeds
    (rewritten so a later `open` sends what it holds now) and the live widgets
    drawing it (`(host, id)`, dropped when the host recycles the widget).

  **The carrier is fixed when the source is made, and that is a finding, not a
  simplification.** The host does not apply a live `path` change: its two doors
  are `/gui_set data` (inline samples only — "the samples are now these") and
  `/gui_set reload` ("they are where they were, and they moved"). So a spilled
  source keeps its own path for life and `set` rewrites that file and re-reads
  it, while an inline one sends the samples. Handing an inline source more than
  `INLINE_MAX` samples raises rather than silently switching carriers under a
  widget that was built around one. A source that *names* samples it does not
  own — a `buffer`, a `cache` — refuses `set` and offers `reload`.

  **What it does not cover.** Only the signal family's samples. The same object
  is the right shape for the other heavy props — a roll's `notes`, a curve's
  `points`, a patcher's `boxes`/`cords`, a score's `display_list` — but those are
  normalized by their builders (`_flat_points`, `_flat_notes`) *before* the node
  exists, so a source there is a change in each builder rather than the one
  expansion point `node` gives for `data`. Written into "Future directions"
  rather than half-done here.

  Docs: the client book's "Where the samples come from" leads with the source and
  keeps the carrier table for naming one by hand. Example: `window.py` drops
  `blob=0` and its positional blob, and gains the redraw — 8000 floats is past
  the inline ceiling, so it spills and the `/gui_def` carries no samples at all.
  Tests: `tests/test_view.py` — the carrier choice, the spill, one source in two
  views, the live push, a freed widget leaving the live ends, the rewrite-and-
  reload of a spilled source, and the two refusals.

- ✅ **C41 — `view()` is the root, and a root with no parent is a window**
  *(done 2026-08-23)*. `window()` is renamed `view()` and the distinction
  "window vs container" becomes positional: a view with a parent is a component,
  a view with no parent is the window. `open()` works on *any* node.

  ```python
  view(layout(knob(a), knob(b)), title="voice").open()   # a titled window
  layout(knob(a), knob(b)).open()                        # a window of two knobs
  knob(a).open()                                         # a window that is a knob
  ```

  **The tradeoff, stated because it is not free**: the *wire* type stays
  `"window"` — `Host::window_defs` keys the renderable document by window-rooted
  def id (`host/mod.rs`, `if node.kind == "window"`), and a non-window root lives
  only in the generic registry and is not drawn as a window. So `GuiHost.open`
  **frames** a non-window root in a `"window"` node client-side, with `hug=True`
  so the frame adds nothing but the OS window the wire needs. The frame is
  invisible: it takes the root id, the content becomes its one child, and the
  handle goes on resolving the tree's names. It is done in `GuiHost.open` rather
  than in `View.open` so the low-level door behaves the same. `view()` is then
  what you write when the window's own properties matter — a title, a size, a
  theme — since those belong to a root nobody frames. `window` stays as an alias
  of `view`.

  **A module named `view` cannot coexist with a builder named `view`.** `View`
  had been given its own `clausters/gui/view.py` in C39; `from clausters.gui
  import view` then resolves to the *submodule*, not the function, and binding
  the name in `__init__` on top of it is a shadowing that depends on import
  order. `View` moved into `guidef.py`, which is where every builder that returns
  one already lives, and the submodule is gone. (`crates/clausters-gui/src/tree.rs` is the
  Rust side of the same job, if a separate module is ever wanted again — the name
  to use is `tree`, not `view`.)

  **`into=` is deferred, and here is why**: there is no wire verb that adds a
  child to a live widget — `/gui_def id json` builds a *whole* tree and
  re-sending an id redefines it, dropping pending edits and reassigning ids. So
  `windowA(into=windowB)` can only be "insert into B's view and redefine B",
  which is correct at build time and lossy at run time. Build-time nesting is
  what C39 already gives (`view(a, b)`); a live `into=` needs a host verb and
  belongs to the GUI track, named there rather than faked here.

  **The editor joins the same shape**, which was listed below as a thing this
  track must not leave behind: `Editor.open`, `open_signal` and `open_pianoroll`
  took the host as a required positional — the last resource that had to be
  handed one. They now default to the ambient host like everything else.

  Docs: the client book's GUI page leads with the root rule and the three
  spellings; the composition page's editor opens with no argument. Examples: the
  32 `gui_*.py` that built a tree now say `view(`, and `bind.py` drops its
  wrapper entirely — one knob is all that window has to show, so the window *is*
  the knob (checked by ear: the bare root still binds and drives the synth).
  Restyling the rest of the examples to `v.open()` is C45's pass, not this one.
  Tests: the frame's shape and invisibility, a lone control opening, and the
  alias (`tests/test_view.py`); the editor's ambient host
  (`tests/test_gui_editor.py`).

#### Part 2 — the GUI and the audio server

- ✅ **C42 — A widget is built from the control it drives** *(done 2026-08-23;
  **corrected the same day**, see below)*. A `knob`, `slider`, `number` or
  `toggle` takes a def's control positionally and reads its **name** — what
  `/node_set` addresses — and its **default**, so the widget and the graph
  cannot disagree about what `"freq"` is and nobody types the name twice.

  ```python
  freq = control("freq", 220.0)
  sd = SynthDef("voice", out(0.0, sine(freq=freq) * 0.2))

  knob(freq, min=110.0, max=880.0)              # name and value, from the control
  slider(sd["amp"], min=0.0, max=1.0, label="level")     # or indexed off the def
  view(*[knob(c, min=0.0, max=1.0) for c in sd.controls]).open()
  ```

  `sd["freq"]`, `fd["cutoff"]` and `gd["mix"]` all give a `ControlInfo`, which is
  the unifying move: a `SynthDef` keeps the `Control` objects its graph
  references (`_controls`, the same first-seen order `spec` walks); a `FaustDef`
  reads its own payload; a `GraphDef` port answers with the targets it drives.

  **The correction, decided by the user the day it shipped: the range does not
  belong to a def.** It first landed as `control(..., min=, max=, step=)` and
  `GraphDef.port(..., min=, max=)`, with the widget reading them. That is wrong
  twice over:

  - **A control is a signal.** It says what value flows into a graph, not how a
    knob should be drawn — and the two names collide outright, because `min`/`max`
    on a signal are the **binary operators** (`freq.min(other)` composes a
    `BinaryOpUGen`). The attributes shadowed the methods for *every* control,
    ranged or not, since one without a range set them to `None`. Nothing in the
    client called them, so no test saw it; TypeScript's compiler refused the
    override outright, which is how it was found.
  - **A GraphDef port is the same category**: a name the server takes any float
    for, declared by hand for the sake of a GUI. It had no range before this
    milestone either, so removing it touches nothing older.

  So `min`/`max` are **spelled on the widget**, and a control that has no range
  of its own says so rather than being drawn over a guess. The one control that
  arrives with a range is a **Faust** parameter, and that is not an exception
  this client makes: `hslider(label, init, min, max, step)` cannot be written
  without one, the compiled DSP reports it back, and `ControlInfo` has carried
  the three fields all along with only that family filling them. Faust's syntax
  showing through, not a range clausters declares.

  **What the host cannot draw, written down rather than faked.** `props::Range`
  is `{value, min, max, label, text_size}`: linear, no step, no curve. A **named
  spec** (`spec="freq"` → 20..20000 *exponential*) was deliberately **not**
  shipped — a spec that silently drew linear would be worse than none. That is
  one entry in `crates/clausters-gui/PLAN.md`, and it survives the correction unchanged:
  it was always about what a *widget* can express.

  The props-parity manifest learned that `control` is **not a prop**: it is a
  *source* of `name`/`value`, so it is a parameter each client spells its own way
  rather than a key on the wire. (That test caught it, which is what it is for.)

  Docs: the defs page gains "A widget is built from the control it drives", the
  GUI page the same section from the widget's side. Example: `bind.py`
  declares `FREQ`/`AMP` once and each widget spells the range it is turned over.
  Tests: `tests/test_control_range.py` — the three families answering one shape,
  a control taking no range at all (and its operators still composing), Faust
  bringing its own, the keyword override and the two refusals.

- ✅ **C43 — The binding is made against the control, not against a widget id**
  *(done 2026-08-23)*. A widget and a def control used to meet only as a string
  typed twice (`win["freq"].bind("/node_set", synth.id, "freq")`), and nothing
  checked that the control existed. A view built from control objects already
  knows which control each widget drives, so the whole surface binds in one verb.

  ```python
  synth = Synth("voice", server=server)

  w = view(knob(freq), slider(amp)).open()
  w.bind(synth)         # one /gui_bind per control widget:
                        # /node_set <synth> <control> <value>
  w.unbind()
  ```

  `knob`/`slider`/`number`/`toggle` keep the control they were built from on the
  node (`View._control`, client-side, never on the wire), the id walk collects
  `widget id -> control name` alongside the names it already collected, and the
  handle carries it — refreshed in place on a redraw, like the names, so a
  rebound window is never wiring ids that recycled. `WindowHandle.controls`
  reports what is there.

  **The name and the control are two different things**, usually spelled the
  same. The widget's `name` is the handle's index; the control name is what the
  server is told. `knob(freq, name="pitch")` binds `pitch` to `freq`, and an
  explicit `name=` is taken out of the props before the control's own is
  applied — which is the collision that showed up the moment the test asked for
  it.

  It is still `/gui_bind` underneath, and the low-level form stays for anything
  that is not a def control (a bus, another widget, an arbitrary address).
  `bind` takes a `Node` or a bare id, and refuses a window where **no** widget
  was built from a control, which can only be a mistake. **Two widgets on one
  control is legal and drifts**: both bind, both set the node, neither is told
  when the other moves, and the host fires an apply rather than a second
  binding. That is the user's inconsistency to make, not the client's to detect;
  it is documented, not guarded.

  Docs: the GUI page's binding section. Example: `bind.py` grew a second
  control (`amp`) precisely so the one verb has more than one thing to wire, and
  binds the panel with `win.bind(synth)`. Tests: in
  `tests/test_control_range.py` — the surface bound in one verb, a widget named
  apart from its control, `unbind`, the empty-window refusal, a node or a bare
  id, and a redraw leaving the window bindable.

- ✅ **C47 — The host this handle did not start: `attach`, and a session that
  takes one** *(done 2026-08-23)*. The question W24 could not port until the
  reference answered it: the page reaches a host by three names (`guiHost()`,
  `newGuiHost()`, `Session.connectGui(url)`), and `connectGui` is a verb this
  client never had. Answering it by renaming the three would have invented a
  fourth rule; the answer is that **the host already had the server's pair, with
  half of it unnamed**. `GuiHost(host, port)` was an attach that did not verify,
  and `boot` was the only verb.

  What landed:

  - **`GuiHost.attach()`**, the peer of `Server.attach()` — verify, connect,
    adopt the ambient registration (`adopt_ambient`, first-wins), own no
    process. `stop` already read ownership off `_process`, so an attached host
    is left standing, windows and all; the verb only had to stop lying about
    having checked. `clausters.launch.gui_is_up` is the probe, the `server_is_up`
    of the visual side: `/gui_query 0` answered by `/gui_info`, which a host
    replies to even for a widget id it does not have.
  - **The probe goes over UDP whatever carrier the handle then uses**, because
    UDP is the one leg the host cannot turn off (`--no-tcp` exists, no
    `--no-udp` does). So it says *the front is bound*, not *your carrier is up* —
    a host started with `--no-tcp` answers the probe and then refuses a
    `transport="tcp"` connection. Written into the docstring rather than
    papered over.
  - **A supplied `interface` skips the verification** (`_own_carrier`), the same
    line `Server.attach` draws: a carrier this module does not know about may
    reach a host that answers no UDP probe.
  - **`Session(server, gui=host)`** — the visual half of taking a `Server` the
    session did not boot. Not a verb: the session already accepts a server
    through the constructor and has no `attach_server`, so the GUI gets the same
    door. `gui()` then returns that host and launches nothing, which is the
    idempotence it already had, stated for a host it was handed.
  - **`Session.connectGui(url)` falls**, and it falls without being replaced:
    it did *connect* and *adopt* in one call, and both halves now exist
    separately and symmetrically with the server's. Recorded for the port in
    `clients/web/PLAN.md` (W24).

  What this does **not** decide, deliberately: whether a host may be remote.
  It may — `GuiHost` is a plain OSC client to any address — but the useful
  topology (host and server together where the screen and the device are, script
  anywhere) is a **bind** question, and the binds are the server's and the
  host's, not this client's. It went to `crates/clausters-gui/PLAN.md` and `PLAN.md`
  instead. Two things would degrade under it and are written down there: the
  `path` carrier of a `Source` (the client writes a temp file the host mmaps —
  one filesystem assumed, so remote means inline-only under the 2 KB ceiling)
  and `--data-dir` (the GuiDef store is the host's).

  Tests: in `tests/test_gui_host.py` — the refusal, the supplied carrier that is
  not probed, the process it does not own, ambient adoption first-wins with
  `stop` giving it up, and a session taking a host it did not boot. Example:
  `examples/panels/attach.py`, which plays both parts in one run (boot, attach a
  second handle with an `IdShare`, a window from each, and the guest letting go
  while both windows stay). Docs: the GUI half of "Several servers, and the one
  you did not start" in `docs/src/sessions.md`, and `attach` named beside `boot`
  in `docs/src/gui.md`.

- ⬜ **C44 (analysis, not scheduled) — the inverse direction: a widget inside a
  def**. Faust's model, where `hslider` *is* the control declaration, suggests
  `play(sine(freq=knob(min=110, max=880)))` — the widget coerced into a control
  and its window opened by `play`. Recorded as a direction with a reservation,
  not as work: it inverts the dependency (`defs` would import `gui`, where
  today the arrow runs the other way and deliberately so — the arrangement's
  `gui → form` rule is the precedent), and it autogenerates the control's name,
  which is the one thing `/node_set` addresses by. If it is done, the coercion
  runs in **one direction only** (`knob → Control`, never a `Control` growing a
  widget) and `name=` is mandatory.

- ✅ **C45 — The examples pass: rewritten against the new surface, and organized
  while they are open** *(done 2026-08-24)*. Every milestone in this track
  changed how an example is *written*, and nothing runs them — not CI, not any
  build — so they were rewritten by hand and by eye.

  **The layout, decided here and applied to both clients.** One folder per
  subject, the same set in `clients/python/examples/` and
  `clients/web/examples/`, so a script and its page sit in the same place under
  the same name: `basics/`, `spectral/`, `buffers/`, `transport/`, `io/`,
  `faust/`, and the three that need a display — `panels/` (controls and layout),
  `views/` (reading something) and `editors/` (writing something), plus a
  web-only `components/` for the surface a page has and a script cannot. The
  `gui_` prefix is gone: it was a taxonomy typed into 37 filenames, and it named
  a folder that now exists. Both READMEs and both books' `examples.md` say what
  each folder holds and nothing else — no catalogue, which is the rule.

  **The `gui_` prefix came off the def names too**, where it had stopped naming
  anything (`gui_bind_beep` → `bind_beep`), and `refresh-bin.sh` learned to
  resolve a bare example name through the folders.

  **Restyled to the surface the track built.** The 28 examples that still said
  `gui.open(view(…))` say `view(…).open()` — the one thing C41 explicitly left
  here — and `panels/shell.py`, which typed `"freq"`, `"amp"` and their two
  defaults **three times each** (in the graph, in the widgets, in the value the
  script remembered), now declares two controls and builds the widgets from
  them. That was the last example in the directory still disagreeing with C42.

  **Read as pairs, verb by verb**, which the flat directories had made
  impossible: 19 pairs, 47 scripts with no page, 12 pages with no script. Two
  pairs turned out not to be pairs (`panels/standalone` authors a bundle in one
  client and boots one in the other; `transport/sync`'s page has a playhead the
  script lacks) and both went to `clients/web/PLAN.md` rather than being
  patched. One pair had never matched by name at all — `scope.html` was the page
  of `meters.py` — and the page took its twin's name, which also frees `scope`
  for the port `views/scope.py` still has no page for.

  **`_curve.html` is gone.** A scratch page left behind by the curve work: no
  header comment, no prose, nothing referencing it, two clips queried and
  logged. It was not an example, and an example directory is not where a
  debugging page lives.

  What this milestone deliberately did **not** do: add a catalogue of examples
  to any book, and touch the repository-root `examples/`, which drives the
  server and moves with the server's own track.

#### What this track must not break

- **Non-divergence.** Every milestone here lands in the web client in the same
  commit or leaves `clients/web/PLAN.md` naming the shape the port must follow.
  `View` is a plain object over the same JSON in both languages; the name index,
  the duplicate-name error and the id-per-instance rule are the same rule
  written twice, and if either turns out to be numeric it belongs in
  `clausters-core` instead.
- **The ambient verbs keep their meaning.** `clausters.plot`/`clausters.scope`
  are *verbs that open a window*; `guidef.plot`/`guidef.scope` are *builders
  that return a node*. That collision predates this track and gets worse once
  the builder's return value can also `open()` — one of the two names has to
  give, and this track decides which rather than leaving both.
- ✅ **`Editor` joins the same shape.** `Editor.open(host)` was the one place a
  resource took the host as a required positional; it resolves the ambient host
  like everything else now (landed with C41).
- **`name` means two things and should not.** On a root it is the *persistence*
  name the host stores for `/gui_load`; on a child it is the client's index.
  `define(name)` should carry the first so the prop is left meaning only the
  second — or the two are named apart. Decide it in C39, before either spelling
  spreads.
- **The examples are the acceptance.** 29 of the `gui_*.py` examples address
  widgets by name and one holds a builder's return value; when this track is
  done, the two spellings are one, and the pair of every ported example is read
  side by side, verb by verb.

## Organization conventions

- Native crates under `crates/`; per-language clients under `clients/<lang>/`. The **core C-ABI is the only contract** between Rust and each language, with an explicit ABI version (as `embed.rs` / `clausters.py` already do).
- Project-wide boundary rule: "only flat data crosses" (bytes/`array`/scalars/integers), in both the transport and the core.
- Client milestone track prefixed with `C` so it does not collide with the server's `M` track; close each one with the project's milestone checklist (code+tests, a clear commit message, this plan's checkbox, developer/user docs where applicable, an example in `examples/` — which is also the manual-test surface for new human-audible/visual behavior —, a `docs/decisions.md` note only for a non-obvious choice).
- Code/comments/tests in English; the `PLAN.md` roadmaps and `docs/decisions.md` in English; conversation with the user in Spanish.
- Documentation is **three mdBooks, one per platform** (server `docs/`, Python client `clients/python/docs/`, web client `clients/web/docs/`), cross-linked by their Read the Docs URLs; the Python user doc for a client milestone goes in `clients/python/docs/`, and its API reference is generated from docstrings by pydoc-markdown. **No Sphinx/RST directives in docstrings and no milestone labels in any published doc** (labels live only in the `PLAN.md` roadmaps).

## Verification

- **Workspace**: `cargo build` and `cargo test` (without features and with `--features faust`) must pass; `crates/clausters-server/tests/rt_safety.rs` and `crates/clausters-server/tests/denormals.rs` stay green after the refactor.
- **Numeric parity**: a new test in `clausters-core` (or `tests/`) comparing the native builtin's output against the server's native branch (bit-exact) and against Faust (tolerance).
- **Client**: `pytest` of the package; smoke of `_native` (builtin, TempoClock, bundle, render).
- **E2E** (CLAUDE.md rule: server and client in the **same** Bash invocation): start `./target/debug/clausters &`, define a FaustDef from the high-level client, `/synth_new`, control via bus, verify `/done`/replies, `kill`. NRT: score generated by `seq` → `render()` → compare WAV/golden.

## Found by use: the running list of fixes and open questions

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Every entry is a **checkbox**, so what is open reads as open at a glance and
nothing has to be inferred from where it sits. A fixed one is ticked, with the
record of what was wrong and why the fix is the shape it is, and moves whole to
[`docs/history/plans/python.md`](../../docs/history/plans/python.md) ("Found by use: the entries that were fixed"): this list holds what is
open, and that file is where a search for a fixed one ends.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ⬜ Acceptable equivalence level for higher math vs Faust (a concrete tolerance).

- ⬜ Whether a separate `cdylib` for `clausters-ffi` is preferable, or exposing its C-ABI from the same `libclausters` (initial preference: separate, so as not to couple client and server embed).

- ⬜ The FFI-overhead threshold at which the scalar builtin uses a pure-language fallback instead of crossing the boundary.

## Future directions (a design that is not a fix)

Every entry carries a checkbox, like "Found by use" above: an open direction has
to read as open, and one that converges into a milestone leaves this list rather
than being ticked here.

An entry that was done without becoming a milestone is ticked and moves whole
to [`docs/history/plans/python.md`](../../docs/history/plans/python.md) ("Future directions: the entries that were done"), so this list
holds what is open.
