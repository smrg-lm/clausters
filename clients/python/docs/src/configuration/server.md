# The server's configuration

Every key of the configuration file's **`[server]`** section, with its type, its
default and the command-line flag that overrides it. The reader is the
`clausters` binary — including the one a `Session.live()` or `Session.embed()`
boots for you — so these keys shape the server behind your session even though
the Python side never reads most of them.

Where the file lives, how the user and project layers merge and which keys the
Python client itself reads are on the [Configuration](../configuration.md) page.
The precedence for everything below is:

```
command-line flag  >  project clausters.toml  >  user config.toml  >  built-in default
```

Every key is optional, and an unknown key is ignored — a file written for a
newer build stays readable by an older one.

## `[server]`

| Key | Type | Default | Flag | What it sets |
| --- | --- | --- | --- | --- |
| `workers` | integer | `0` (the server chooses) | `--workers` | DSP worker threads for `/g_parallel` groups |
| `sample_rate` | integer (Hz) | `48000`; `0` follows the device | `--sample-rate` | The output rate the server imposes on the audio backend |
| `audio_buses` | integer | `128` (also the hard maximum) | `--audio-buses` | Audio bus count; buses `0..outputs` are the hardware outs |
| `control_buses` | integer | `16384` | `--control-buses` | Control bus count |
| `taps` | integer | `8`; `0` disables the tap region | `--taps` | Audio-tap rings in the shared segment — what `/tap` routes an audio bus into, and what an oscilloscope reads |
| `tap_frames` | integer (samples) | `16384`, rounded up to a power of two | `--tap-frames` | Per-tap ring capacity |
| `outputs` | integer | the device's own channel count | `--outputs` | Hardware output channels |
| `inputs` | integer | `0` (no input device) | `--inputs` | Hardware input channels; `> 0` opens the default input device, readable with `In` on buses `outputs..outputs+inputs` |
| `max_nodes` | integer | `8192` | `--max-nodes` | Node slab capacity, the root group included |
| `max_buffers` | integer | `4096` | `--max-buffers` | Buffer pool size |
| `max_graph_children` | integer | `512` | `--max-graph-children` | Per-group child capacity |
| `max_ugen_inputs` | integer | `32` (also the maximum) | `--max-ugen-inputs` | Accepted inputs per UGen when a def is compiled |
| `persist` | boolean | `true` | `--no-persist` | Reload the def store on boot and write new defs to it |
| `data_dir` | string (path) | `$CLAUSTERS_DATA_DIR`, else the XDG data dir | `--data-dir` | Where the def store (`defs/`, `midi.json`, `boot.json`) lives |
| `shm` | string (path) | off | `--shm` | The shared-memory segment local clients map (put it on `/dev/shm`) |
| `tcp` | boolean or port | `true` — on at `57110`, beside UDP | `--tcp [port]` / `--no-tcp` | Length-prefixed OSC over TCP |
| `ws` | boolean or port | off; `true` means `57120` | `--ws [port]` | OSC over WebSocket, reachable from a browser |
| `max_frame` | integer (bytes) | `16777216` (16 MiB) | `--max-frame` | Largest OSC frame on the stream transports (TCP and WebSocket) |
| `max_clients` | integer | `64` | `--max-clients` | Concurrent stream clients, TCP and WebSocket combined |
| `midi` | boolean or string | off; `true` means `"clausters"` | `--midi [name]` | Virtual MIDI input port, by name |

Three keys are **toggles that may carry a value** — `tcp`, `ws` and `midi`:
`true` enables the transport at its default port (or name), `false` disables it,
and a number (a string, for `midi`) enables it at that specific port (name). TCP
is the one transport on by default, so its meaningful setting is `false`.

Two keys size a **slab built once at boot** rather than a live limit:
`max_nodes` and `max_buffers` fix what the server can hold for its whole run, and
raising them costs memory whether or not you use it.

## Flags with no configuration key

| Flag | What it does |
| --- | --- |
| `--pin <cpu[,cpu…]>` | CPU affinity (Linux, experimental, `rtprio` builds only): the first CPU takes the audio callback thread, the rest are round-robined over the DSP workers |
| `-v` / `-vv` / `-vvv` / `-q` | Log verbosity: warn (the default) → info → debug → trace, `-q` for errors only. `RUST_LOG` overrides it, and a client retunes it live with `/verbosity` |

The offline renderer is a separate mode with its own flags and no configuration
section, since a render is described by its invocation:

```sh
clausters --nrt score.osc out.wav --rate 48000 --channels 2 --format float --workers 0
```

`--format` is `int16`, `int24` or `float` (the default). From Python the same
render is `Session.nrt()`, which never shells out to these flags.

## What the Python client takes from this section

`ServerOptions()` reads `audio_buses`, `control_buses`, `sample_rate`, `outputs`,
`inputs`, `max_nodes`, `max_buffers`, `max_graph_children` and `max_ugen_inputs`
as its own defaults — they size the client's bus allocators as well as the
server it launches. The behavior keys (`workers`, `tcp`, `ws`, `midi`, `persist`,
`max_frame`, `max_clients`) have no client-side counterpart: `ServerOptions`
carries them as `None`, emits no flag, and lets the server's own precedence
stand. Set one explicitly and it becomes a flag, which wins over the file.

```python
from clausters import Session, ServerOptions

Session.live()                                    # every key above from the file
Session.live(options=ServerOptions(ws=True))      # ...except --ws, forced on here
```

## A worked file

```toml
# ~/.config/clausters/config.toml — the machine's defaults
[server]
sample_rate = 48000
outputs = 2
inputs = 2               # a duplex card: In reads buses 2..4
taps = 16                # room for eight stereo oscilloscopes
shm = "/dev/shm/clausters"
midi = "studio"          # the virtual port shows up under this name
```

```toml
# ./clausters.toml — this project, overlaying the above
[server]
control_buses = 4096     # a small piece; the rest of the machine's file stands
persist = false          # this project's defs are built from source every run
```
