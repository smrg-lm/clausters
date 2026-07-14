# Configuration

Clausters reads a single TOML configuration file, shared by the server and every
client. It is **read-only** to the programs: you edit the files; the programs
never write them. Machine-written state (the def store, `boot.json`, `midi.json`)
lives separately, under the data directory.

The file sets defaults only. A value passed on the command line — or to a client
constructor, or to a server launched with explicit flags — always wins over the
file.

## Where the files live

Two layers combine, the **project** layer overriding the **user** layer:

1. **User** — the first of:
   - `$CLAUSTERS_CONFIG` (a direct path to the file),
   - `$XDG_CONFIG_HOME/clausters/config.toml`,
   - `%APPDATA%\clausters\config.toml` (Windows),
   - `~/.config/clausters/config.toml`.
2. **Project** — the nearest `clausters.toml` found by walking up from the
   current working directory, the way Cargo finds `Cargo.toml`. The first match
   wins; the search stops at the filesystem root.

## Runtime paths (what the programs write)

The config file above is only read. Everything a program writes at runtime lives
elsewhere:

| What | Default path | Overrides |
| --- | --- | --- |
| Def store (`defs/`, `midi.json`, `boot.json`) | `$XDG_DATA_HOME/clausters`, else `~/.local/share/clausters` | `--data-dir` / `[server].data_dir` / `$CLAUSTERS_DATA_DIR`; disable with `--no-persist` / `[server].persist = false` |
| Shared-memory segment | `/dev/shm/clausters_<pid>_<n>` (else `$TMPDIR/...`); removed on exit | `--shm` / `[server].shm` |
| Faust standard library (read only) | searched under `$FAUST_PREFIX/share/faust`, then `~/.local/share/faust`, then `/usr/local/share/faust` | `$FAUST_PREFIX` |

Def persistence is **on by default**: a server — including one a Python session
launches — reloads its def store on boot and writes new defs to it, so the only
directory the library creates under your home is `~/.local/share/clausters`. Pass
`--no-persist` (or `persist = false`, or a throwaway `data_dir`) for a server
that writes nothing.

## Precedence

From highest to lowest:

```
command-line flag  >  project clausters.toml  >  user config.toml  >  built-in default
```

The merge is field by field: a key absent from the project file keeps the user
file's value, and a key absent from both falls back to the program's compiled
default. Unknown keys are ignored, so a newer file stays readable by an older
build.

## Schema

Every key is optional; an absent key takes the built-in default. Sections are
grouped by audience.

```toml
[server]
workers = 0              # DSP worker threads; 0 lets the server choose
sample_rate = 48000      # imposed output rate in Hz; 0 follows the device
audio_buses = 128        # audio bus count
control_buses = 1024     # control bus count
taps = 8                 # audio-tap rings for oscilloscopes; 0 disables
tap_frames = 16384       # per-tap ring capacity in samples (a power of two)
persist = true           # persist/reload defs; false is like --no-persist
# data_dir = "/path"     # def store location (else the XDG data dir)
# shm = "/clausters"     # shared-memory segment path for local clients
# tcp = true             # TCP transport (on by default at the OSC port):
#                        # false disables it, a port number moves it
# ws = 57120             # WebSocket transport: true = default port, or a number
# max_frame = 16777216   # largest OSC frame on TCP/WebSocket, in bytes
# midi = "clausters"     # virtual MIDI input: true = default name, or a name

[client]                 # the Python (and future) client
host = "127.0.0.1"       # server host
port = 57110             # server UDP port
latency = 0.0            # seconds added to each event's timetag
clock = "sample"         # Session.live() clock timebase: "sample" (default, the
#                        # server's sample clock — drift-free) or "monotonic"
#                        # (wall-clock timetags). Falls back to wall-clock if no
#                        # server answers; embed and render/nrt stay wall-clock.
# transport = "tcp"      # command carrier: "tcp" (default), "udp" or "ws"

[gui]                    # the GUI host
host_port = 57210        # port for the host's script-facing front (UDP + TCP)
# tcp = true             # the front's TCP leg (on by default at host_port):
#                        # false disables it, a port number moves it
# max_frame = 16777216   # largest OSC frame on the TCP leg, in bytes
# server = "127.0.0.1:57110"  # also attach the client leg to this audio server
# shm = "/clausters"          # map this segment for meters/scopes
# data_dir = "/path"          # GuiDef store location

[standalone]             # the self-contained app (GUI + embedded server)
gui = "drone"            # the saved GuiDef to open when --standalone has no name
boot = true              # run the GuiDef's boot messages and boot.json preset
# data_dir = "/path"     # bundle location
```

The `tcp`, `ws` and `midi` keys are toggles that may also carry a value:
`true` enables the transport at the program's default port or name, `false`
disables it, and a number (or, for `midi`, a string) enables it at that specific
port or name. TCP is the one transport that is **on by default** (at the OSC
port, alongside UDP), so its `true` is the implicit state and `false` (or
`--no-tcp`) is the meaningful override.

## Per-program use

- **Server** — the `[server]` section supplies the defaults for every flag of
  `clausters` (`--workers`, `--sample-rate`, `--audio-buses`, `--control-buses`,
  `--taps`, `--tap-frames`, `--tcp`/`--no-tcp`, `--ws`, `--max-frame`,
  `--midi`, `--shm`, `--data-dir`, `--no-persist`). A flag on
  the command line overrides the file.
- **GUI host** — the `[gui]` section supplies the defaults for `clausters-gui`
  (`--port`, `--tcp`/`--no-tcp`, `--max-frame`, `--server`, `--shm`,
  `--data-dir`, `--headless`); the `[standalone]`
  section supplies the standalone launch. `clausters-gui --standalone` with no
  name opens `[standalone].gui`. A `--config <path>` flag reads one specific file
  instead of the user+project chain.
- **Python client** — the `[client]` section provides the defaults for
  `Session.live()` / `Server` (`host`, `port`, `latency`, `transport`, `clock`);
  the `[server]` section provides the `ServerOptions` defaults (`audio_buses`,
  `control_buses`, `sample_rate`). `clock` picks a `Session.live()`'s default
  timebase — `"sample"` anchors to the server's sample clock, `"monotonic"`
  keeps wall-clock timetags (embed and offline stay wall-clock). See the Python
  client's own documentation.

## The standalone app

With a bundle saved under a data directory (a GuiDef plus its SynthDefs,
GraphDefs, FaustDefs and an optional `boot.json`), the GUI can launch the whole
thing with no language interpreter:

```sh
clausters-gui --standalone --data-dir /path/to/data
```

The embedded audio server loads the data directory's defs and `boot.json`, the
GuiDef's own `boot` messages run, and its window opens. Set `[standalone].gui` so
the name can be omitted, and `[standalone].data_dir` so `--data-dir` can be too —
then the launch is just `clausters-gui --standalone`.
