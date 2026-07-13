# Configuration

The client reads the same TOML configuration file the server and the GUI read,
so all three agree on one place for defaults. The full schema, the file
locations and the precedence rules are documented once, in the server
documentation's **Configuration** chapter; this page covers what the Python
client takes from it.

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
  latency = 0.0
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
  control_buses = 1024
  sample_rate = 48000
  # Boot-time hardware channels and pre-allocated pools:
  outputs = 2          # omit to follow the device default
  inputs = 0           # >0 opens the input device; In reads it
  max_nodes = 1024
  max_buffers = 1024
  max_graph_children = 256
  max_ugen_inputs = 32
  ```

  `ServerOptions()` then reads those values; a field passed to the constructor
  still wins. `ServerOptions.args()` turns them into `clausters` CLI flags
  (`--outputs` only when set), so a server launched from the object matches it;
  `Server.query_info()` reads the same fields back from a running server
  (including `input_channels` and the pool sizes) as a `ServerInfo`.

## Reading it yourself

`clausters.config.load_config()` returns the merged configuration as a nested
dict (cached after the first call; pass `refresh=True` to re-read). It is the
same loader the `Server` defaults use, so you can inspect exactly what they will
see.

## Python version

The loader uses the standard-library `tomllib`, so the client requires **Python
3.11 or newer**.
