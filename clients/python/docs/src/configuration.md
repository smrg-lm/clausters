# Configuration

The client reads the same TOML configuration file the server and the GUI read,
so all three agree on one place for defaults. This page covers what the Python
client takes from it; the two programs a session launches have a page each,
key by key:

- [The server's configuration](configuration/server.md) — the `[server]`
  section, its command-line flags and the offline renderer's own.
- [The GUI host's configuration](configuration/gui.md) — the `[gui]` section
  plus the `[gui.theme]` and `[gui.metrics]` role tables (the host's whole look
  and sizing) and `[standalone]`.

The file locations and the precedence rules are also stated in the server
documentation's **Configuration** chapter, which covers the same file from the
server's side.

The client only **reads** the file — it never writes it. A value passed
explicitly in code always wins over the file, which in turn wins over the
built-in default.

## What the client reads

Two sections feed the client's defaults:

- **`[client]`** — connection defaults for `Session.live()` and `Server`:

  ```toml
  [client]
  host = "127.0.0.1"
  port = 57110          # one number serves UDP and TCP alike
  # latency = 0.1       # RT lead; unset -> 0.1 (real-time transports), 0.0 (NRT)
  # transport = "tcp"   # the command carrier: "tcp" (default), "udp" or "ws"
  ```

  With these set, `Session.live()` (no arguments) connects to `127.0.0.1:57110`
  with the configured latency. Passing `host`, `port` or `latency` explicitly
  overrides the file:

  ```python
  Session.live()                 # uses [client] from the config
  Session.live(host="otherhost") # host explicit, port/latency from the config
  ```

- **`[server]`** — the defaults for `ServerOptions` (which sizes the client's bus
  allocators and emits matching launch flags):

  ```toml
  [server]
  audio_buses = 128
  control_buses = 16384
  sample_rate = 48000
  # Boot-time hardware channels and pre-allocated pools:
  outputs = 2          # omit to follow the device default
  inputs = 0           # >0 opens the input device; In reads it
  max_nodes = 8192
  max_buffers = 4096
  max_graph_children = 512
  max_ugen_inputs = 32
  ```

  `ServerOptions()` then reads those values; a field passed to the constructor
  still wins. `ServerOptions.args()` turns them into `clausters` CLI flags
  (`--outputs` only when set), so a server launched from the object matches it;
  `Server.query_info()` reads the same fields back from a running server
  (including `input_channels` and the pool sizes) as a `ServerInfo`. Its live
  counterpart is `Server.status()`, a `ServerStatus`: what the server is doing
  now rather than what it was built with — how many synths, groups, UGens and
  defs it holds, the audio thread's average and peak share of the block budget,
  and the blocks that have missed it since boot. Both print as a readable
  block.

  `ServerOptions` also carries the server's **behavior options** — `workers`,
  `tcp`, `ws`, `midi`, `persist`, `max_frame`, `max_clients`, `pin` — which
  have no client-side counterpart: their default `None` emits no flag, so the
  server's own precedence (CLI flag > project config > user config > compiled
  default) stays intact, and a set value emits the flag, which wins. E.g.
  `Session.live(options=ServerOptions(ws=True, workers=3))` launches a server
  that also listens on WebSocket and runs three DSP workers.

## Reading it yourself

`clausters.config.load_config()` returns the merged configuration as a nested
dict (cached after the first call; pass `refresh=True` to re-read). It is the
same loader the `Server` defaults use, so you can inspect exactly what they will
see.

## Python version

The loader uses the standard-library `tomllib`, so the client requires **Python
3.11 or newer**.
