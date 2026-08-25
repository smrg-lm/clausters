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
| **iOS Safari caps wasm memory near 350 MB** | Undocumented, but reproducible: the tab crashes above it. | Keep the buffer pool modest on mobile; a long take is streamed rather than pooled. |

## Not yet — each one owned

These are absences with a milestone against them, and they are listed here so
that a reader who hits one today knows which.

| What | Status |
|---|---|
| **`/def_send faust` reaches a native server only** | The in-page engine carries no Faust compiler. Closed by `B5` + `W7`. |
| **`diskIn`/`diskOut` are not built** | They stream the server's own filesystem, which the wasm build has none of, so a def naming them fails cleanly as an unknown UGen. A tab does have a private filesystem (OPFS) — see the B track. |
| **`/buffer_allocRead` has no file to read** | Samples enter through the host instead: the page decodes and hands the engine the frames. Same B-track work. |
| **Buffer building runs on the audio thread** | The native server has a thread for it; the in-page engine has not had one, so a large allocation is paid where the audio is. Same B-track work. |
