# The example pairs

Most Clausters examples exist twice: a script under `clients/python/examples`
and a page under `clients/web/examples`. The non-divergence rule in the
repository's `CLAUDE.md` says the two are **one example in two languages** —
same material, same names, *the same calls to the same API in the same order* —
and until this file existed the only thing enforcing that was somebody reading
both files. Every divergence ever found was found by accident.

`scripts/audit-example-pairs.py` reads both files instead. It extracts each
one's **ordered sequence of calls on the client surface** — every call whose
name is one either client declares, in source order, with a named callback
inlined where it is handed over rather than where it is written — and compares
the two. Everything else (`mkdir`, `getElementById`, `toFixed`, a local helper,
a list method, the arithmetic TypeScript spells as `.mul()`) never enters the
sequence; the script's header says exactly what is dropped and why.

What is left over after that is what this file declares. It is the same posture
`bindings.md` takes for the ABI: the interesting question is never *are the two
identical*, it is **is each difference one somebody decided?** A difference with
a row here is a decision; a difference without one is a defect, and the audit
fails on it.

Run it before committing a change to either example directory:

```sh
scripts/audit-example-pairs.py                 # every pair
scripts/audit-example-pairs.py views/rulers    # one pair, by name
scripts/audit-example-pairs.py --verbose io    # with both sequences printed
```

It is not a CI job — nothing in this repository runs an example, and this does
not run one either; it reads them. `docs/contributing.md` lists it with the
other checks a person runs by hand.

## The pairs that are not spelled alike

A pair is found by its path: `views/rulers.py` is `views/rulers.html`, with
`_` becoming `-`. These are the ones where that does not hold.

| Python example | Page | Why |
|---|---|---|
| `panels/standalone` | `panels/standalone.mjs` | the page beside it is not this example's other half: `standalone.html` is the counterpart of *running* `clausters-gui --standalone`, in a tab. Authoring a bundle is a node script's job, because a page never writes one |
| `io/osc_responder` | `io/responders.html` | the page is named for what it holds, since the other responder example a page can run (`io/midi-responder.html`) sits beside it |

## Examples with no twin

An example with no counterpart is a gap unless there is a reason it cannot have
one. Each row says which.

| Example | Why |
|---|---|
| `buffers/render_then_load` | renders a take to a **file** and loads it back through `/buffer_read`, a round trip through a path; a tab has no filesystem, and `buffers/offline-render.html` shows the same take reaching the engine as samples |
| `editors/session` | drives the GUI host as a **separate process** over the session's transport, which is what a page has instead of, not as well as: its host is a canvas in the same page |
| `io/embedded` | the in-process embedded server through the bundled native library — the page's engine is that same server compiled to wasm, so every page is this example |
| `io/live_udp` | a UDP socket to a server process; a tab has no UDP, and `io/servers.html` shows the same two-server split over its own carriers |
| `io/osc_destination` | sends OSC to a **foreign** application over UDP; same reason |
| `views/recording_mapped` | the same three pictures as `views/recording.html` on a host that **maps the segment**, so none of them crosses the wire. A page can map nothing — that is what the example is about, and it is the one thing a tab cannot do |
| `views/bulk` | maps a multi-gigabyte take through the bulk cache with an mmap; a page fetches ranges instead, which `views/take.html` shows |
| `basics/demand.html` | demand-rate UGens, written page-first while the def DSL was being ported; the script twin is named in `clients/python/PLAN.md` |
| `basics/engine.html` | booting the wasm engine in an AudioWorklet — the page's half of `io/embedded.py`, with no script counterpart for the same reason |
| `basics/synth.html` | the smallest page there is: one def, one node, one slider. It is the web book's opening example and has no script twin by design |
| `components/authored.html` | the `<clausters-bundle>` custom element over an authored bundle — a browser element, so there is nothing to port |
| `components/demo.html` | as above |
| `components/lifecycle.html` | as above |
| `editors/document/index.html` | the document host as a page application, with its own bundle |
| `panels/piano/index.html` | the **other** mapping path: a bundle whose keyboard is bound to a `voice` the host instances, with no script between the key and the sound. `panels/piano.html` is the twin of `panels/piano.py`, which programs the voices from the `note` events |
| `panels/piano/make_bundle.mjs` | the authoring half of the bundle above, the way `panels/standalone.mjs` authors its own |
| `panels/graph-controls/index.html` | a bundle page, like `components/*`: the script side is the `make_bundle.mjs` beside it |
| `panels/host.html` | the GUI host on a canvas in this page — what `editors/session.py` is on the other side |
| `panels/standalone.html` | a standalone bundle booted with no server; `panels/standalone.py` authors one, and the two are a pair the audit reads through `panels/standalone.mjs` |
| `panels/two-hosts.html` | two hosts in one page, which is a page's shape of the two-window example |
| `transport/sequencing.html` | the sequencing overview page of the web book; the script side is spread over `transport/timeline.py` and `basics/tempo_canon.py` |
| `views/window.html` | opening a host window from a page, the page's half of `panels/shell.py` |

## The idiom table

The same capability, spelled the way each language spells it. A row pairs two
names; a row with `—` on one side drops the other, because that side has
nothing to pair it with. The verdicts read as they do in `bindings.md`:
`idiom` (shaped for the language, nothing to do), `n/a` (deliberately absent,
with the reason), `gap` (present on one side and nobody has decided).

| Python | Web | Note |
|---|---|---|
| `Session.live` | `Session.embed` | `idiom` — the real-time session. A script boots a server process and talks to it over a socket; a page *is* the process, with the engine in an AudioWorklet. `Session.live` exists in the web client too, for a `--ws` server, and a page that means that one says so |
| — | `widget` | `idiom` — addressing a widget by name. Python subscripts the window (`win["wave"]`), which is no call at all; TypeScript has no `__getitem__`, so it spells the same reach as a method and the audit drops it |
| `close` | — | `n/a` — teardown. A script must give back what it opened before it exits; a page's session, server and host live as long as the tab, and the ones that do close something (an offline render's session, say) are not saying anything the script's last lines say. Dropped on **both** sides for that reason: keeping it on one would invent a difference wherever the other happens to tear down |
| — | `close` | `n/a` — the other half of the row above |
| `wait` | — | `n/a` — the script's ending. A script *blocks* on the window so the process does not exit while it is on screen; a page is already alive, and where it awaits the same verb it is a promise, not a stop. Dropped on **both** sides, for the same reason `close` is |
| — | `wait` | `n/a` — the other half of the row above |
| — | `formatWidgetInfo` | `idiom` — a `WidgetInfo`'s readable line. Python's is the record's own `__str__`, reached by printing it, which is no call; TypeScript has no `__str__`, so the same text is the free function `gui.formatWidgetInfo` |
| — | `loadCore` | `n/a` — the shared core's wasm module, which has to be loaded before the first call into it. A script needs no such line: ctypes opens its library on first use |
| `Score` | `open` | `idiom` — a score is engraved when it is built, and libverovio in a page is fetched asynchronously; a TypeScript constructor cannot await, so the same act is the static factory `Score.open` |
| — | `call` | `idiom` — applying a Faust box to its inputs. Python's `Box` is callable, so an unapplied fragment is wired up by writing `box.faust("os.osc")(freq)`; TypeScript has no callable object and spells the same application `.call(freq)` |
| `read` | `fromSamples` | `idiom` — loading a take into a server buffer. A script renders it to a `.wav` and the server reads the path (`/buffer_read`); a tab has no path, so the samples go straight in |
| `segments` | `tempoSegments` | `idiom` — every segment of a tempo map, in order. Same reason as the row below: the wasm `TempoMap` cannot grow a method, so the wrapper takes the map |
| `env` | `tempoEnv` | `idiom` — writing a tempo envelope onto a map. The Python client's `TempoMap` carries the method; the wasm one is a handle with no room for it, so the same act is the free function `tempoEnv(map, …)` |
| — | `at` | `idiom` — the last element of a list. Python subscripts it (`upper[-1]`), which is no call at all; JavaScript has no negative index and spells the same reach `.at(-1)` |
| — | `sub` | `idiom` — a number minus a signal. Python's `__rsub__` makes `1.0 - spread` an operator like any other; TypeScript has no reversed operator, so the free function takes both operands |
| — | `keys` | `idiom` — looping over a mapping's keys. `for k in mapping` yields them in Python; a JavaScript `Map` yields entries, so the same loop says `.keys()` |
| `IdShare` | — | `idiom` — the slice of an id space two clients divide. Python's is a class that validates its pair; TypeScript's is an **interface**, so the same value is the object literal it describes (`{ index: 0, of: 2 }`) and there is nothing to construct |
| `samples_to_file` | `source` | `idiom` — handing a take to a view. A script writes a `.f32` file the host maps and names the path; a tab has no filesystem, so a page hands the samples straight over as a `gui.source`. One call either way, in the same place |

## What one side of a pair says alone

Per pair, and only for what the platform makes impossible on the other side. A
row is a decision; anything not listed here is a defect the audit reports. A
row drops the **last** call of that name on that side, since what is declared is
almost always the file's ending; `×2` in the call cell drops the last two, and
`(first)` counts from the start instead.

The right answer is almost always to **fix the example**, not to add a row —
the pairs below are the ones where fixing it is not available. Most of them say
the same thing, and it is worth saying once here: **a script has an ending and a
page does not.** A script that opened a window blocks on it and then gives back
what it took — it frees the node it made, stops the host it launched, closes the
session — because the process is about to exit and nothing else will. A tab
holds all of that until it is closed, and a page that spelled the same teardown
would be tearing down the thing the reader is still looking at. `close` and
`wait` are dropped globally for that reason; the verb each script ends on
(`free`, `stop`) is named per pair, because those same verbs are ordinary work
in the middle of an example.

### `panels/oscsend`

| Only in | Call | Why |
|---|---|---|
| python | `free` | the script's ending: it frees the synth it made before closing the session |

### `panels/skeleton`

| Only in | Call | Why |
|---|---|---|
| python | `stop` | the script's ending: it launched a headless `clausters-gui` process and stops it before exiting. The page's host is the tab |

### `panels/stack`

| Only in | Call | Why |
|---|---|---|
| python | `stop` | the script's ending, as in `panels/skeleton` |

### `panels/workspace`

| Only in | Call | Why |
|---|---|---|
| python | `stop` | the script's ending, as in `panels/skeleton` |

### `views/nodetree`

| Only in | Call | Why |
|---|---|---|
| python | `free` | the script's ending: it frees the group it made before closing the session |

### `views/plot`

| Only in | Call | Why |
|---|---|---|
| python | `stop` | the script's ending, as in `panels/skeleton` |

### `views/scope`

| Only in | Call | Why |
|---|---|---|
| python | `free` | the script's ending: it frees the synth it made before closing the session |

### `views/window`

| Only in | Call | Why |
|---|---|---|
| python | `stop` | the script's ending, as in `panels/skeleton` |

### `transport/conductor`

| Only in | Call | Why |
|---|---|---|
| python | `unfollow_transport` | the script's ending: its `close()` cell unfollows the shared transport and closes both clients and the conductor before the process exits |

### `io/midi_responder`

| Only in | Call | Why |
|---|---|---|
| web | `requestMidiPorts` | a browser has no **virtual** port to create: the script opens one named `clausters-in` and asks the user to route into it, where a page must ask the browser for access and pick from the ports that already exist |
| web | `stop` (first) | the port picker above re-listens, which stops the receiver it had. Only a page has that picker |
| web | `free` | the page's stop button frees the responders it registered; the script's process exit does that for it |

### `editors/patch2`

| Only in | Call | Why |
|---|---|---|
| python | `GuiHost` | a script launches a `clausters-gui` process to draw into and closes it after; the page's host is the tab, and `plotDef` resolves the ambient one |
| python | `boot` | the other half of the row above |
| web | `onClosed` | the script blocks on `PatchWindow.wait` until the view is closed and then opens the other one. A page may not block, so the same sequencing is the callback |

### `io/servers`

| Only in | Call | Why |
|---|---|---|
| python | `Server` | the refused second boot: the script boots a handle on a port that is already busy to show that `boot` will not adopt a server it did not start. A tab has no ports and each `boot` is an engine of its own, so there is no busy address to collide with |
| python | `boot` | the other half of the row above |

### `panels/bind`

| Only in | Call | Why |
|---|---|---|
| python | `free` | the script's ending: it frees the synth the panel drove before closing the session |

### `faust/boxes_library`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `wavBytes` | the script's `render` writes the take to the path it was given and says `pw-play out.wav`; a tab has no path, so the same take leaves as a download |

### `panels/panel`

| Only in | Call | Why |
|---|---|---|
| python | `stop` | the script's ending, as in `panels/skeleton` |

### `panels/shell`

| Only in | Call | Why |
|---|---|---|
| python | `set` | the script's ending: it closes the gate on whatever voice is still sounding before the session goes. Closing the tab takes the engine with it |

### `views/take`

| Only in | Call | Why |
|---|---|---|
| web | `stop` (first) | the script hands the button the bound method (`on_click(editor.stop)`); a JavaScript method reference loses its receiver, so the page wraps the same call in an arrow |

### `basics/envelope`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `spectral/convolution`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `spectral/cross`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `spectral/kernel`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `transport/freeze`

| Only in | Call | Why |
|---|---|---|
| web | `stop` (first) | the page's stop button ends the routine that drives the piece; the script's process exit ends it |

### `basics/typed_controls`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `basics/graph_maths`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `basics/multichannel`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `basics/wavetables`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `basics/pause_resume`

| Only in | Call | Why |
|---|---|---|
| python | `read_soundfile` | the beat RMS is read off the file the server wrote, because the script asked `render` for a path. The page's render answers with the take itself, and reads the same numbers off that |
| python | `channel` | the other half of the row above |
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `panels/attach`

| Only in | Call | Why |
|---|---|---|
| python | `gui_is_up` | the launcher's probe: it asks whether a host **process** is still answering on a port after the guest handle let go. A page has neither a process nor a port -- its host is the tab, and the two windows on screen are the same answer |

### `editors/composed`

| Only in | Call | Why |
|---|---|---|
| web | `stop` ×2 | the script hands each stop button the bound method (`on_click(editor.stop)`); a JavaScript method reference loses its receiver, so the page wraps the same call in an arrow |

### `editors/pianoroll_midi`

| Only in | Call | Why |
|---|---|---|
| web | `requestMidiPorts` | a browser has no **virtual** port to create: the script opens one named `clausters-in` and asks the user to route into it, where a page must ask the browser for access and pick from the ports that already exist |
| web | `stop` (first) | the port picker above re-listens, which stops the receiver it had. Only a page has that picker |
| python | `free` ×2 | the script's ending: it frees the two `MidiFunc` responders before the process exits |
| python | `stop` | the script's ending: it stops the receiver it opened |

### `editors/multitrack`

| Only in | Call | Why |
|---|---|---|
| web | `events` | a clip spec's event builder. The script keeps the specs as dicts and subscripts one (`spec["events"](...)`, no call of its own); an object literal reads it as a property, so the same builder is reached by name |

### `transport/automation_lane`

| Only in | Call | Why |
|---|---|---|
| web | `Server` | the page's `pw-play`. The script renders to a path and prints where the WAV went; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `buffers/offline_render`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |

### `views/meters`

| Only in | Call | Why |
|---|---|---|
| python | `send_msg` | `/buffer_allocRead`: the script writes a WAV to a temp file and asks the server to read it. A tab has no filesystem, so the page makes the samples and writes them into the buffer it allocated |
| web | `setSamples` | the other half of the row above |
| web | `onClosed` | the script's animation loop asks `win.closed` on every turn, which is a property and no call at all; a page's interval has to be told when to stop |

### `spectral/chain`

| Only in | Call | Why |
|---|---|---|
| web | `deactivate` | the page's ending: it gives up the ambient session when the render is done, where the script's process exit does it |
| web | `Server` | the page's `pw-play`. The script renders to a path and prints `listen with: pw-play out.wav`; a tab has neither a path nor a shell, so hearing the take is booting an engine, putting the samples in a buffer and playing that |
| web | `boot` | the other half of the row above |
| web | `fromSamples` | the other half of the row above |
| web | `play` | the other half of the row above |
| web | `wavBytes` | the take, saved. `render(path=...)` writes the file for the script; a page has no path, so the same bytes leave as a download |
