# Examples

Runnable demos live in `examples/` (Rust and Python) and `clients/python/`. Unless noted, the Python ones need the server running first (`cargo run --release`) and use the standard library only.

## Rust examples (`cargo run --example <name>`)

| example | what it shows | run |
|---|---|---|
| `osc_ping` | Minimal OSC client for manual testing: subcommands `status`, `beep`, `vibrato`, `map` (the `/n_map`/`/n_mapa` demo), `quit`. | `cargo run --example osc_ping -- beep` |
| `bench` | Graph-throughput benchmark (offline): how many copies of a graph fit in real time at 48 kHz, plus the parallel-group speedup. | `cargo run --release --example bench` |
| `render_golden` | Regenerates the golden reference WAVs in `tests/golden/` from the shared scenes — run it and **listen** before committing. | `cargo run --example render_golden` |

## Python clients (`examples/`)

| script | what it shows |
|---|---|
| `json_client.py` | Generates defs as JSON and drives the server over OSC. Subcommands: `status`, `ugen`, `faust`, `wavetable`, `buffer`, `bundle`, `score` (writes an NRT score to `/tmp/clausters_score.osc`). |
| `auto_order.py` | [Auto-sorted groups](auto-order.md): builds a source → fx → master chain reversed on purpose and repairs it with one `/g_sortMode`. |
| `sample_clock.py` | [The sample clock](sample-clock.md) as master timebase: models `sample(t)` from `/clock` anchors and schedules with `/sched`. |
| `shm_client.py` | The shared-memory transport (`--shm`): the same OSC with no sockets anywhere. |
| `clock_recorder.py` | The shared-memory sample clock made checkable: reads `ShmClient.clock` directly (no round trip), schedules a pristine `Impulse` **exactly every N samples** with `/sched`, records the real output (`pw-record`) and reports the impulse spacing/jitter/drift. Duration is free (`--seconds`), seconds to hours. |
| `embed_render.py` | Synchronous offline render through the embed C ABI (`ctypes`), no server process. |

The OSC encoding/decoding in these scripts is hand-rolled (stdlib only); they double as a compact reference for the wire format.

## Python binding (`clients/python/clausters.py`)

The reusable client library (standard library + `ctypes`): a shared-memory client and the embed façade with a synchronous `request()` call. See [Local transports & embedding](ipc.md) for the API.
