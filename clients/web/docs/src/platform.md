# What a tab cannot do

The two clients are one client in two languages: a verb, a class or a figure
that exists in one and not the other is a defect, and the port catching up is
the normal fix. This page is for the other case — the places where the
**platform**, not the port, is the reason, so that "missing" reads as a decision
somebody took rather than as work nobody did.

The rule for reading it: a row here is a limit of the browser. Anything not
here and not in the Python client is a gap, and gaps live in
`clients/web/PLAN.md`, not on this page.

## Permanent

Nothing scheduled closes these. They are properties of the browser as a place
to run an audio server in.

| What | Why | What happens instead |
|---|---|---|
| **Parallel groups do not run in parallel** | DSP threads mean wasm threads, wasm threads mean `SharedArrayBuffer`, and that is only exposed to a **cross-origin isolated** document (`COOP: same-origin` + `COEP: require-corp`). A component that embeds on someone else's page cannot ask its host for headers. | `/group_parallel` is accepted, the group is marked and reported by `/group_queryTree`, and its children run in child order — the `workers = 0` path a native server also takes by default. Stages are bit-identical to sequential by construction, so the **samples are the same**; only the wall-clock time is missing. `Session.embed` therefore takes no `workers` argument here. |
| **No flush-to-zero** | wasm has no FTZ/DAZ mode. The native engine arms it on every processing thread. | A signal that enters the denormal range costs more in a tab than in a window; it does not sound different. |
| **Sample values may differ by an ULP** | `wasm32` uses Rust's own libm; native lowers to the system's. | Native↔wasm render parity is a tolerance (max delta 1e-6), not bit-identity — see `docs/decisions.md`. Same-platform RT ≡ NRT stays exact. |
| **A buffer cannot be a mapped file** | A page cannot map a file into memory. | The engine's own memory holds it, and `/buffer_get`/`/buffer_getRange` carry it out — the same split every bulk path already has. |
| **A page cannot bind a port** | Nothing can address a tab. | A responder registers against a **carrier** the client already opened (`src` is a socket URL or `page`), not a `(host, port)` pair. |
| **Audio needs a user gesture** | Autoplay policy. | `Session.embed` is called from a click, and the components carry the affordance (`<clausters-power>`). |
| **iOS Safari caps wasm memory near 350 MB** | Undocumented, but reproducible: the tab crashes above it. | The engine reserves 16 MB at boot and is linked with a 256 MB ceiling, so it grows into a budget rather than into that crash. Keep the buffer pool modest on mobile; a long take is streamed rather than pooled. |

## Not yet — each one owned

These are absences with a milestone against them, and they are listed here so
that a reader who hits one today knows which.

| What | Status |
|---|---|
| **`diskIn` streams WAV only** | A span of a compressed file is not a file, so it cannot be decoded on its own; a WAV span can be, and is. Reading a compressed file whole is what a buffer is for (`Buffer.read`). Natively `diskIn` reads whatever the decoder does. |
| **A stream starts after a longer lead** | Natively a thread shares a ring with the audio thread. In a tab there is no shared memory, so a Worker reads a span and *moves* it across a port — how far ahead it reads is the design, not a tuning constant, and an underrun is silence exactly as a slow disk gives. |
| **`/buffer_read` is not delegated** | Its sibling `/buffer_allocRead` is: it leaves the AudioWorklet for the NRT worker and comes back decoded. This one overlays a file onto the buffer's *current* contents, which live in the engine's own memory, so the job cannot leave without shipping them out and back. It runs in the worklet, under the serving budget. |
| **`/buffer_write` has nowhere to write** | Reading is done; writing a file back out is the other half and is not built. |

## What a tab has that it did not

Kept here because the absences above were listed as permanent-looking for a
while and two of them were not.

**A tab has a filesystem**, its own: the origin private file system. So
`Buffer.read(path)` — `/buffer_allocRead` — means something in a page, and the
path is `/`-separated under the origin's root. The file is read and decoded by
the **NRT worker**, a thread beside the audio one, with the *server's own*
decoder rather than the browser's, so the samples are bit-for-bit the ones a
native read of the same file gives. `Buffer.load(url)` is the other door and a
different thing: a file over the network, decoded by the browser.

**A tab compiles Faust**, in all three forms. `libfaust-wasm` runs in the NRT
worker — the page's compiler thread, the way a native server has one — and the
module it emits is instantiated inside the engine's *own* linear memory, with
its `compute` in the engine's function table and its transcendentals bound to
the engine's own libm. So a Faust node is a node: `/node_set` by name, group
order, bus summing, done actions, all the same code as in a window. A page whose
bundle carries no FaustDef never fetches any of it.

A def built from the **signal** or **box** API is read by the server's own
interpreters, compiled to wasm and running in that Worker — not by a second
reading of the schema written for the browser. Faust's own Emscripten bindings
expose neither API, so the vendored compiler is built with them added
(`third_party/build-faust-wasm.sh`); what a page carries is the same compiler a
window links, at the same pin.

**A tab streams to and from that filesystem.** `diskOut` records into it while
the take plays and `diskIn` streams back out of it, neither holding the whole
thing — the two reductions above are the price, and they are in the table rather
than left to be heard.

**Buffer work is no longer paid in the audio callback.** A long take arrives in
runs (a tenth of a second of stereo at a time) and becomes visible in one swap,
and the jobs that can leave the audio thread do.
