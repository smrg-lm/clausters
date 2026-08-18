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
port = 57110             # base OSC port: UDP binds it, TCP follows it
workers = 0              # DSP worker threads; 0 lets the server choose
sample_rate = 48000      # imposed output rate in Hz; 0 follows the device
audio_buses = 128        # audio bus count
control_buses = 16384    # control bus count
taps = 8                 # audio-tap rings for oscilloscopes; 0 disables
tap_frames = 16384       # per-tap ring capacity in samples (a power of two)
persist = true           # persist/reload defs; false is like --no-persist
# data_dir = "/path"     # def store location (else the XDG data dir)
# shm = "/clausters"     # shared-memory segment path for local clients
# tcp = true             # TCP transport (on by default at the base port):
#                        # false disables it, a port number moves it
# ws = 57120             # WebSocket transport: true = the base port + 10,
#                        # or a number
# max_frame = 16777216   # largest OSC frame on TCP/WebSocket, in bytes
# max_clients = 64       # concurrent stream clients, TCP + WebSocket combined
# midi = "clausters"     # virtual MIDI input: true = default name, or a name

[client]                 # the Python (and future) client
host = "127.0.0.1"       # server host
port = 57110             # server UDP port
latency = 0.0            # seconds added to each event's timetag
clock = "sample"         # real-time session clock timebase: "sample" (default,
#                        # the server's sample clock — drift-free) or "monotonic"
#                        # (wall-clock timetags). Falls back to wall-clock if no
#                        # server answers; applies to Session.live() and
#                        # Session.embed(); render/nrt stay wall-clock.
# transport = "tcp"      # command carrier: "tcp" (default), "udp" or "ws"

[gui]                    # the GUI host
host_port = 57210        # port for the host's script-facing front (UDP + TCP)
# tcp = true             # the front's TCP leg (on by default at host_port):
#                        # false disables it, a port number moves it
# ws = true              # the front's WebSocket leg (off by default): true for
#                        # host_port + 10 (57220), or a port number —
#                        # browser-reachable, the same toggle as the server's
# max_frame = 16777216   # largest OSC frame on the stream legs (TCP and
#                        # WebSocket), in bytes
# server = "127.0.0.1:57110"  # also attach the client leg to this audio server
# shm = "/clausters"          # map this segment for meters/scopes
# data_dir = "/path"          # GuiDef store location
# font = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
#                             # the typeface to draw text with, on a host built
#                             # with a rasterizer (its `font-atlas` feature)
# follow_block = 1.0          # seconds of recorded material a picture waits for
#                             # before it re-reads its summary: a recording is
#                             # drawn in blocks, and larger is cheaper and
#                             # choppier (0 follows every frame). Neither the
#                             # sound nor a playhead reads it

[gui.theme]              # color-role overrides for the host's look (optional)
# accent = "#ff8c40"     # each entry is role = "#rrggbb" or "#rrggbbaa"; the
# text = "#e8e8ec"       # table is partial — unlisted roles keep the default
# field = "#20222a"      # theme. Role names are the host's Theme fields
#                        # (accent, text, field, background, selection, ...);
#                        # an unknown role warns and is skipped, never fatal

[gui.metrics]            # size-role overrides for the host's sizing (optional)
# scale = 0.9            # the density: one multiplier regenerating the whole
#                        # table (below 1 compact, above 1 comfortable)
# pad = 6                # each entry is role = number, in logical pixels (glyph
# gap = 8                # scales for the text roles); the table is partial —
# control_h = 26         # unlisted roles keep their generated default. Role
#                        # names are the host's Metrics fields (pad, gap,
#                        # margin, control_h, track_thick, header_w, ruler_h,
#                        # point_radius, hit_slop, text_scale, caption_scale,
#                        # ...); an unknown role or an unusable number warns and
#                        # is skipped, never fatal

[standalone]             # the self-contained app (GUI + embedded server)
gui = "drone"            # the saved GuiDef to open when --standalone has no name
boot = true              # run the GuiDef's boot messages and boot.json preset
# data_dir = "/path"     # bundle location
```

The `tcp`, `ws` and `midi` keys are toggles that may also carry a value:
`true` enables the transport at the program's default port or name, `false`
disables it, and a number (or, for `midi`, a string) enables it at that specific
port or name. TCP is the one transport that is **on by default** (at the base
port, alongside UDP), so its `true` is the implicit state and `false` (or
`--no-tcp`) is the meaningful override.

`port` is the **base** the others are measured from: UDP binds it, TCP follows
it and WebSocket sits ten above, so moving one number moves the whole server and
several run side by side, one per port. A transport that should sit somewhere
else says so with its own key (or `--udp`/`--tcp`/`--ws` on the command line);
UDP is the one that cannot be turned off, since it is the door a client probes
to find the server at all. The virtual MIDI port's default name follows too — a
server off 57110 opens `clausters:<port>` rather than a second `clausters`.

## Per-program use

- **Server** — the `[server]` section supplies the defaults for every flag of
  `clausters` (`--port`, `--workers`, `--sample-rate`, `--audio-buses`,
  `--control-buses`, `--taps`, `--tap-frames`, `--tcp`/`--no-tcp`, `--ws`,
  `--max-frame`, `--max-clients`, `--midi`, `--shm`, `--data-dir`,
  `--no-persist`). A flag on the command line overrides the file. `--udp` has no
  key of its own: in a file, write the base in `port` and give `tcp` a number.
- **GUI host** — the `[gui]` section supplies the defaults for `clausters-gui`
  (`--port`, `--tcp`/`--no-tcp`, `--max-frame`, `--server`, `--shm`,
  `--data-dir`, `--headless`); the `[standalone]`
  section supplies the standalone launch. The `[gui.theme]` table restyles the
  host's color roles; `--theme <path>` lays a free-standing theme file — the
  same flat `role = "#rrggbb[aa]"` table — over it, so a look can be shared as
  one file. Both are partial overlays over the built-in dark theme, and the
  same table goes on cascading over the wire: a GuiDef container may scope a
  further overlay to its subtree (a **theme group**, the `theme` prop) and any
  widget may re-seed its accent with the `color` prop — see [the GUI
  protocol](gui-protocol.md). The `[gui.metrics]` table is the sizing
  counterpart: the same partial-overlay semantics over the host's **size roles**
  (spacings, control thicknesses, the chrome strips, the text scales), whose
  defaults are generated from one quantized scale over the font cell. Its
  reserved `scale` key is the whole density surface — one multiplier
  regenerating the table, since a host has one density the way it has one look,
  so there is nothing to set per widget and nothing on the wire. Its numbers are
  **logical** pixels, like the wire's own: each window resolves the table to its
  display's physical pixels once, when its scale changes.
  `clausters-gui --standalone` with no
  name opens `[standalone].gui`. A `--config <path>` flag reads one specific file
  instead of the user+project chain.
- **Python client** — the `[client]` section provides the defaults for
  `Session.live()` / `Server` (`host`, `port`, `latency`, `transport`, `clock`);
  the `[server]` section provides the `ServerOptions` defaults (`audio_buses`,
  `control_buses`, `sample_rate`). `clock` picks a real-time session's
  default timebase (`Session.live()` and `Session.embed()` alike) — `"sample"`
  anchors to the server's sample clock, `"monotonic"` keeps wall-clock timetags
  (offline stays wall-clock). See the Python client's own documentation.

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
