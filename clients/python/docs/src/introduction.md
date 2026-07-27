# clausters - Python client

`clausters` is the reference high-level **Python client** for the [Clausters](https://clausters.readthedocs.io/) audio server, ported selectively from SuperCollider's class library ([sc3](https://github.com/smrg-lm/sc3)). It covers both of the server's definition formats as peers: UGen-graph **SynthDefs** and **FaustDefs** — and a FaustDef, in turn, is built equally from the signal API, the box API or Faust source.

It is **pure Python at runtime**: it reaches the shared native core through `ctypes` over the `clausters-ffi` cdylib, and speaks ordinary OSC bytes to the server (UDP, TCP, shared memory, or an in-process embedded server). A NumPy user can wrap a returned `array('f')`, but NumPy is never a dependency — only flat data crosses the binding.

This is the **package documentation**. The server itself — the OSC protocol, the def formats (SynthDef JSON and Faust), the node-tree model, the C ABI contract, and how to embed it — is documented separately in the **[Clausters server book](https://clausters.readthedocs.io/)**; this site links to it rather than repeating it. The **[web client book](https://clausters-web.readthedocs.io/)** is the third: the same client model in a browser, where this client's threads become promises and the server can run in the page itself. One book per platform.
<!-- Cross-links to the companion books; update the URLs if the Read the Docs slugs differ. -->

## Components

- A **server-agnostic** timing and value layer (`clausters.base`): scalar/list math backed by the same core the server uses (so results match by construction), operator overloading, the `Routine`/`yield` coroutine layer, a `TempoClock` that does timing only, and a choice of timebase (monotonic or the server's sample clock).
- **Definitions and server resources** (`clausters.defs`): the UGen graph (`ugens` + `SynthDef`) and `FaustDef` (from `signals`, from `boxes`, or from Faust source), plus the `Node`/`Bus`/`Buffer` handles. The `Server` owns the communication interface and emits through it — swap its interface to retarget the *same* code from a live RT take to an offline NRT score.
- **Sequencing** (`clausters.seq`): `Event`, the value patterns (`Pseq`, `Pwhite`, `Pseries`, ...) and `Pbind`, and `EventStreamPlayer`, with yield-exact timing.
- **Ergonomic defaults without global state** (`clausters.Session`): bundles a `Server` and a clock; several sessions (e.g. an offline one for plots next to a live one) coexist in one script.
- **The ambient verbs** (`play` / `plot` / `render`, plus the live `scope`): one free-standing entry each for sounding, showing and bouncing whatever you have — an event, a pattern, a bare signal expression, a def, a timeline, a buffer — resolved against the ambient session.

## The RT / NRT / embed seam

The key design property is a single seam: the `Server` holds a communication interface, and *which* interface it holds decides where the bytes go — a live UDP/TCP server, shared memory, or an in-process embedded engine that renders a score to WAV offline. The clock, the routines and the patterns do not change. The same script can drive a real-time take and render an offline score, with no global build context in between.

## How to read this book

- **New here?** Start with [Getting started](getting-started.md): install the package and make a sound, line by line in a REPL.
- **Want the mental model?** Read [The client, layer by layer](guide.md): `base`, `seq`, `defs` and the seam.
- **Want the ergonomic entry point?** [Sessions](sessions.md) shows how one handle drives a live take or an offline render, and [The ambient verbs](verbs.md) covers `play`, `plot` and `render` — what each accepts and why the split.
- **Driving the clock yourself?** [Routines and clocks](routines-and-clocks.md) writes routines by hand, and [Timing models](timing-models.md) covers the `TempoClock`'s timing modes — wall-clock, sample-locked, shared transport — and how to observe each.
- **Looking for runnable code?** See [Examples](examples.md).
- **Looking for a symbol?** The [API reference](api.md) is generated from the package docstrings.

## License

**GPL-3.0-or-later**.
