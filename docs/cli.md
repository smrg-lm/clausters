# Command-line reference

Two binaries, both built by the workspace (`cargo build --release`, into
`target/release/`) and both bundled in the Python wheel, which puts them on your
`PATH`:

| Binary | What it is |
| --- | --- |
| `clausters` | The audio server. Real time by default; `--nrt` renders a score offline instead. |
| `clausters-gui` | The GUI host — the visual server that owns the windows and draws the widgets. |

Every flag below defaults to the configuration file, and a flag on the command
line wins over it; the keys, the file's locations and the merge order are in
[Configuration](configuration.md). `--help` prints the same tables the binary
carries.

The Python package installs a console script also called `clausters`, which runs
the bundled binary with whatever you pass it — so everything here applies —
**and adds three client-side verbs of its own** (`stop`, `panic`, `status`),
documented in [the Python client book](https://clausters-python.readthedocs.io/).

## `clausters` — the real-time server

```
clausters [--port <n>] [--workers <n>] [--shm <path>] [--data-dir <dir>]
          [--no-persist] [--prune-defs] [--udp [port]] [--tcp [port] | --no-tcp] [--ws [port]]
          [--midi [name]] [--sample-rate <hz>]
```

### Ports and transports

One **base** port carries the server: UDP binds it, TCP follows it, WebSocket
sits ten above. Moving that one number moves the whole server, which is what
lets several run side by side on one machine — see
[Configuration](configuration.md#schema) for the rule in full.

| Flag | Value | Default | What it does |
| --- | --- | --- | --- |
| `--port` | port | `57110` | The base OSC port. |
| `--udp` | port (optional) | the base port | Moves the UDP front alone. UDP cannot be turned off: it is the door a client probes to find the server. |
| `--tcp` | port (optional) | the base port | Length-prefixed OSC over TCP — the command plane, on by default. Bare, it follows the base port. |
| `--no-tcp` | — | — | Disables TCP entirely (a UDP-only server). |
| `--ws` | port (optional) | the base port + 10 | Also accept OSC over WebSocket (`ws://host:port/`), reachable from a browser. Off unless asked for. |
| `--max-frame` | bytes | `16777216` | Largest OSC frame on the stream transports. A denial-of-service ceiling, not a protocol limit; UDP keeps its ~64 KB datagram cap regardless. |
| `--max-clients` | count | `64` | Concurrent stream clients, TCP and WebSocket combined. A connection past the ceiling is dropped at accept; UDP is connectionless and unaffected. |
| `--max-stream-buses` | count | `4096` | Bus indices one `/bus_stream` subscription may list. A subscription is one client's whole live picture — a page of many canvases asks for a bus per meter — so the ceiling scales with a document, not with a widget. What a given client may ask for is this clamped by what one reply carries over its carrier, and `/server_query.reply` reports **that** number to it. |

### Audio

| Flag | Value | Default | What it does |
| --- | --- | --- | --- |
| `--sample-rate` | Hz | `48000` | The output rate the server imposes on the backend; `0` follows the device. |
| `--outputs` | count | the device's | Hardware output channels. Audio buses `0..outputs` are the hardware outs. |
| `--inputs` | count | `0` | Hardware input channels; above zero it opens the input device, readable with `In` on buses `outputs..outputs+inputs`. |
| `--host` | name | the platform's | The audio host to use — `jack`, `alsa`, `pipewire`, `coreaudio`, `wasapi`, whatever this build has. |
| `--device` | name | the host's default | Output device, by exact name or a substring of one. Under JACK it is also the client name the ports carry. |
| `--input-device` | name | the host's default | Input device, by name. Capture belongs to whoever holds this device — an on-demand session cannot record. |
| `--client-name` | name | cpal's own | What the server calls itself to the audio graph, so its ports come back under the same name after a restart and a patchbay can reconnect them. PipeWire reads it from the environment (the only door cpal leaves open); under JACK use `--device`. |
| `--list-devices` | flag | off | Print every host and device this build can see — the names the three flags above take — and exit. |
| `--workers` | count | `0` | DSP worker threads for [parallel groups](parallel.md); `0` lets the server choose. Also accepted by `--nrt`. |
| `--pin` | cpu[,cpu…] | off | CPU affinity: the first for the audio callback thread, the rest round-robin over the workers. Linux, experimental, and only in a build with the `rtprio` feature. |

### Boot-time capacities

Each is a slab built once at startup, not a live limit. A client sizes its own
allocators from these, so a server launched with other numbers is worth
[querying](schemas.md) rather than assuming.

| Flag | Value | Default | What it does |
| --- | --- | --- | --- |
| `--audio-buses` | count | `128` | Audio buses; also the hard maximum. |
| `--control-buses` | count | `16384` | Control buses. |
| `--max-nodes` | count | `8192` | Node slab capacity, the root group included. It also scales the node-id partition every client allocates from. |
| `--max-buffers` | count | `4096` | Buffer pool size. |
| `--max-graph-children` | count | `512` | Per-group child capacity. |
| `--max-ugen-inputs` | count | `32` | Accepted inputs per UGen when a def is compiled; also the maximum. |
| `--taps` | count | `8` | Audio-tap rings for oscilloscopes; `0` disables the tap region. `/bus_tap` routes an audio bus into one. |
| `--tap-frames` | samples | `16384` | Per-tap ring capacity, rounded up to a power of two. |

### Storage, MIDI and logging

| Flag | Value | Default | What it does |
| --- | --- | --- | --- |
| `--shm` | path | off | The shared-memory segment local clients map — put it on `/dev/shm`. A segment that already exists is **attached to**, not truncated: the first server on it owns the command plane and the samples, and any later one plays what the owner published. See [Local transports & embedding](ipc.md). |
| `--data-dir` | dir | the XDG data dir | Where defs are persisted and reloaded. Several servers may share one. |
| `--no-persist` | — | — | Disables def persistence for this run. |
| `--prune-defs` | — | — | Drops the persisted defs that no longer load, instead of warning about them. Only the families this build has are pruned, so a build without a def family never eats its library. |
| `--midi` | name (optional) | `clausters` | Opens a virtual MIDI input port. A server off the default OSC port carries the port in the default name (`clausters:57130`), so two on one machine stay distinguishable. |
| `-v`, `-vv`, `-vvv` | — | warn | Log verbosity: info, debug, trace. `-q` for errors only; `RUST_LOG` overrides both (`RUST_LOG=clausters::osc=trace`). A client retunes it live with `/server_verbosity` and `/server_dumpOsc`. Logs go to stderr. |

A running server stops on `/server_quit` or Ctrl-C.

## `clausters --nrt` — the offline render

```
clausters --nrt <score.osc> <out.wav> [options]
```

The score is the scsynth binary format: OSC packets back to back, each preceded
by its byte count as a big-endian `int32`, with timetags counting seconds from
the start of the render. The render ends at the **last** bundle, so close every
score with one that makes no sound (a final `/node_free`) to set the duration.

| Flag | Value | Default | What it does |
| --- | --- | --- | --- |
| `--rate` | Hz | `48000` | Sample rate. |
| `--channels` | count | `2` | Output channels. |
| `--format` | `int16`, `int24`, `float` | `float` | Output sample format. |
| `--seed` | integer | a fresh one each run | Starting seed for the noise UGens. The seed used is reported, so passing it back replays that exact take. |
| `--stats` | — | — | Print the render's statistics as one JSON line instead of the human summary — for a client driving `--nrt`. |
| `--workers` | count | `0` | DSP threads for parallel groups. Bit-identical output, only faster. |

## `clausters-gui` — the GUI host

```
clausters-gui [--port <n>] [--server <host:port>] [--shm <path>] [--headless]
              [--tcp [port] | --no-tcp] [--ws [port]] [--max-frame <bytes>]
              [--data-dir <dir>] [--standalone [name]] [--config <path>]
              [--theme <path>] [--font <path>] [--msaa <n>]
              [--follow-block <seconds>]
```

The host has two legs: a **server front** a script sends `/gui_*` to, and an
optional **client leg** into a running audio server. What travels over each is
[The GUI protocol](gui-protocol.md).

| Flag | Value | Default | What it does |
| --- | --- | --- | --- |
| `--port` | port | `57210` | The host's own base port for the script-facing front (UDP and TCP). |
| `--tcp` | port (optional) | the host port | The front's TCP leg, on by default; the flag only moves it. |
| `--no-tcp` | — | — | Disables the TCP leg (a UDP-only front). |
| `--ws` | port (optional) | the host port + 10 | Also accept `/gui_*` over WebSocket, reachable from a browser. |
| `--max-frame` | bytes | `16777216` | Largest OSC frame on the stream legs. |
| `--server` | host:port | off | Also attach the client leg to a running audio server. Needed for widgets that reference a server buffer number, and for bound widgets (`/gui_bind`) to forward their value. |
| `--shm` | path | off | Map the audio server's shared-memory segment (its own `--shm` path) for meters and scopes with no per-frame messages. Unix only. |
| `--data-dir` | dir | the XDG data dir | The GuiDef store: named GuiDefs persist there and `/gui_load` reads from it. |
| `--standalone` | name (optional) | — | Boot the saved GuiDef against an **embedded** audio server — no separate server, no language client. With no name, `[standalone].gui` from the configuration is used. |
| `--config` | path | the user+project chain | Read the configuration from this TOML file instead. |
| `--theme` | path | — | Read the color theme from this TOML file, laid over `[gui.theme]`. A flat, partial table of `role = "#rrggbb[aa]"`. |
| `--font` | path | — | Draw text with this typeface (TrueType/OpenType) instead of the embedded bitmap face. Read only by a host built with a rasterizer (its optional `font-atlas` feature); any other build warns and keeps its bitmap face. With the feature and no path, one of the system's own faces is used when there is one. |
| `--follow-block` | seconds | `0` | How much recorded audio a picture waits for before it re-reads its summary. A take being recorded grows with nothing announcing it, so the host follows the buffer's **write frontier** (see [`ipc.md`](ipc.md)) and re-summarizes what appeared. The default follows **every frame**, which is what the picture should do: the work is the block's and not the take's — summarizing a span touches the buckets over it and their parents — so a step costs about five microseconds a channel whatever the take's length. A larger block is cheaper and choppier, and neither the sound nor a playhead over it is affected. |
| `--msaa` | count | `1` (off) | Antialias every window with n-sample multisampling; `4` is the usual smoothing. It is the render pass's attachment that is multisampled, so it costs one attachment per window and nothing per widget; a count this GPU does not offer for the surface format falls back to `1` with a warning. |
| `--headless` | — | — | Run the protocol with no display, for tests and machines with no GPU. The default opens windows. |
| `-v`, `-vv`, `-vvv` | — | warn | Log verbosity, as the server's. |
