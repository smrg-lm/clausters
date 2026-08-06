# Command line

Installing the package puts two commands on your `PATH`, because the wheel
bundles both binaries:

| Command | What it is |
| --- | --- |
| `clausters` | The audio server — plus three client-side verbs this package adds. |
| `clausters-gui` | The GUI host, the visual server that owns the windows. |

Neither has to be built or installed separately, and nothing here is required to
use the library: `Session.live()`, `Server().boot()` and `Session.gui()` launch
the same binaries for you. The command line is for the times there is no script —
or no script *left*.

## Running a server

`clausters` passes everything it is given to the bundled binary, so every server
flag works exactly as documented in
[the server book's command-line reference](https://clausters.readthedocs.io/en/latest/cli.html):

```sh
clausters                          # a server on 57110
clausters --port 57130 --workers 3 # a second one, beside it
clausters --nrt score.osc out.wav  # an offline render
clausters --help                   # the binary's own flags
```

## Acting on a server already running

Three verbs are this package's own — they do **not** launch anything, they talk
to a server that is already up. They are words, while every server flag starts
with a dash, so the two namespaces cannot collide.

| Command | Sends | What it does |
| --- | --- | --- |
| `clausters status [--port <n>]` | `/server_query` | Reports whether a server answers, and how it is configured (rate, channels, buses). |
| `clausters panic [--port <n>]` | `/group_deepFree 0` | Frees every node. The server stays up and keeps its defs, buffers and MIDI bindings — only the sound stops. |
| `clausters stop [--port <n>]` | `/server_quit` | Stops the server. |

`--port` defaults to `57110`; pass it to reach a server booted elsewhere.

The case they exist for is the **stray server**. A client that crashes leaves the
server process running, holding the audio device and quite possibly still
sounding, with nothing left to tell it to stop:

```sh
clausters panic     # silence, but the server keeps everything it has loaded
clausters stop      # or end it outright
```

Without them the only way out is `kill`, which is a blunter instrument than the
server needs: `/server_quit` lets it shut its own device and streams down.

The same three are available from Python as
[`Server.free_all`](api.md#clausters.defs.server.Server.free_all),
[`Server.quit`](api.md#clausters.defs.server.Server.quit) and
[`Server.query_info`](api.md#clausters.defs.server.queries.ServerQueries.query_info), on a
handle from [`attach`](sessions.md#several-servers-and-the-one-you-did-not-start).

**Exit codes**: `0` on success, `1` when no server answers at that address (so
`clausters status` doubles as a probe in a shell script), `2` for a malformed
command line.

## Which binary you get

The launchers and both commands resolve their binaries in one order, and the
first hit wins:

1. the environment override — `CLAUSTERS_BIN` for the server, `CLAUSTERS_GUI_BIN`
   for the GUI host (`CLAUSTERS_LIB` and `CLAUSTERS_FFI_LIB` for the shared
   libraries);
2. the copy bundled inside the installed package;
3. the workspace's `target/` directory, in a source checkout.

An installed wheel only ever sees the second. The override is what points a
source checkout at a build you just made — in a checkout the *bundled* copy still
wins, so it is also what keeps a manual test from silently exercising the binary
that was staged last.
