# Notebooks: the GUI in a cell

Everything else in this guide draws in a window the operating system opens.
A notebook has no such window, and often no such operating system — the kernel
may be on a server across the world. So the GUI goes where you are looking:
into the cell's output area, as WebAssembly.

That is the `clausters-jupyter` package. It is a separate install, and it adds
nothing to `clausters`:

```sh
pip install clausters-jupyter        # pulls clausters of the same version
```

```python
import clausters_jupyter             # once, at the top

plot(sine(440) * 0.5)
scope(bus=0)
```

The import is the whole setup. After it the ordinary verbs draw in cells, and
the client underneath is unchanged — the same `Session`, the same defs, the
same patterns and clocks. A script written against a desktop host runs here as
it is.

## What is actually running where

Two backends, and they differ in capability, not in comfort.

**`page`** (the default) puts both halves in the browser tab: the GUI host and
the audio engine, each compiled to wasm, the engine in an AudioWorklet. The
kernel authors and nothing else. This is the only backend that works with a
remote kernel — JupyterHub, Colab, a remote VS Code — because nothing it needs
lives on the kernel's machine. What it gives up: Faust (the in-page engine is
built without libfaust), shared memory and memory-mapped bulk data, so meters
and scopes read over the wire rather than out of a segment.

**`native`** boots the ordinary `clausters` server on the kernel's machine,
with all of its capability, and keeps only the GUI in the page:

```python
import clausters_jupyter

session = clausters_jupyter.notebook("native")
```

It is local-only, for two separate reasons: the sound comes out of the kernel's
speakers, and the host in the page opens its own WebSocket to the server's
`--ws` port — from the *browser*. Neither reaches you across a network, and
nothing can detect the difference from inside the kernel, so this is a rule
rather than a check. Ask for `native` from a remote kernel and you get a
notebook that draws, silently, whose meters never move.

Either way the GUI is the wasm host, so it is the same widgets, the same
`/gui_*` protocol and the same GuiDefs as a served page or the desktop.

## A window is a cell's output

`plot` returns a window, and the window becomes a canvas when it is
**displayed** — which in a notebook means being a cell's last expression:

```python
plot(sine(220) * 0.5)          # draws here
win = plot(sine(220) * 0.5)    # draws nothing
win                            # draws, in whichever cell this runs
```

That is ordinary notebook behavior rather than a rule of this package: an
object shows nothing until it is shown. It has one useful consequence — a
window opened in a loop does not fill the output with canvases nobody asked
for — and it costs nothing, because a window not yet displayed is remembered
and replayed when it is. The same replay is what rebuilds your canvases after
a page reload or a moved output.

One host serves the whole notebook, however many cells draw. The second cell
joins the host the first booted; it does not load the wasm again.

**Several notebooks in one browser tab are fine.** JupyterLab is a single-page
application, so every notebook open in one tab shares that page — but not a
host: each gets one of its own, and they share only the page's windowing event
loop. Nothing is partitioned between them, so two notebooks may hold the same
widget, node, bus and buffer ids without ever seeing each other, exactly as if
they were open in separate tabs.

The second notebook is cheap: it boots its host on the assets the first one
already staged and on the wasm already compiled, so what it costs is the host
itself. Its audio is its own — a separate `AudioContext` for the in-page
backend — which browsers cap (Chrome at six), and that is the ceiling on how
many sounding notebooks one tab holds.

## What a view decides, and what it does not

A **view** is the most ephemeral thing in a notebook: re-running a cell
disposes one, clearing an output disposes one, closing the tab disposes them
all — while the kernel, which is what a notebook *is*, carries on. So a view
decides what is **drawn** and nothing else. A cell whose output is gone stops
its window's frames, and that is the whole of it; the host, the engine and the
sound follow the kernel.

That is the same split the widget libraries make — `jupyter_rfb` keeps a synced
flag of whether it has visible views and consults it to decide whether to draw
a frame, never to decide what exists — and it is worth stating because the
other model is so tempting: an engine that suspends when the notebook is not on
screen sounds tidy, and it means reopening a tab starts audio nobody asked to
restart, and a re-run of one cell is a hole in the sound.

## What lives as long as what

The kernel is the unit. A notebook's GUI host and its engine live exactly as
long as the kernel that authored them, and the rule is worth stating because
none of the obvious guesses is right:

- **Closing the notebook's tab frees nothing**, and should not: JupyterLab
  leaves the kernel running, so reopening the tab reattaches to it and finds its
  windows and its sound where they were. It also does not *stop* anything —
  a piece playing when you close the tab is still playing, exactly as a script
  whose terminal you hid is still running. `server.quit()` is what stops it.
- **Restarting the kernel silences it immediately** — a synth left running has
  nothing that could stop it once the kernel that knew its id is gone — but
  keeps the host until you come back. The first cell you run afterwards is what
  releases it, because that is the first moment anything can be sure the old one
  is finished with.
- **Shutting the kernel down frees it outright.** Nothing can reattach to a
  kernel that is gone.

What none of this does is free what *you* made. A synth keeps playing and a
window keeps its canvas until something frees them, for as long as the kernel
lives — so a notebook ends the way a script does:

```python
synth.free()
session.gui().close_all()
server.quit()
```

Quitting the in-page engine stops the audio thread and discards what the server
held — its defs, its buffers, its nodes. Nothing restarts it, so the page lets
the whole runtime go and the *next* thing the notebook sends brings up a fresh
one: running the cells again is what fills it, since the client sends its defs
every time. You do not reload the tab.

## Sound needs a click

A browser starts no audio until something in the page is clicked. Any click,
key or touch anywhere in the notebook does it, once — but until then the engine
runs silently, which looks exactly like a piece that has not started.

What it does need is *a cell*, because the page runs nothing at all until some
cell has an output — the front end is served as a widget's module, so with
nothing displayed there is no comm, no wasm and no `AudioContext`. A window is
one; a notebook that only sounds has none, and asks for the engine's own:

```python
clausters_jupyter.audio()
```

An empty box that draws nothing and only has to exist. **You display it**, as
the last expression of a cell, the way every widget library hands you an object
and lets the cell show it — this package puts no output in a notebook you did
not ask it to write into. Send audio with nothing on screen and it does not
appear by itself: the packet waits, and you are told, once, which line to add.

## A bound widget does not involve the kernel

`bind` registers the forward inside the GUI host, and the host talks to the
audio server directly — both in the page under the default backend, and over
that WebSocket under `native`. So a knob turned while a long cell runs still
drives the pitch at frame rate:

```python
win = session.gui().open(window(knob(name="freq", min=110.0, max=880.0)))
win["freq"].bind("/node_set", synth.id, "freq")
```

This is the same wire a served page and the desktop use. The kernel is an
author, never a relay.

## One thing a notebook cannot do

An **unbound** widget sends its value back to the kernel, and that value cannot
arrive while a cell is running. Not slowly — at all. ipykernel holds the shell
channel for the duration of a cell so that cells run in order, and the front
end's message queues behind it. Awaiting does not help; the lock is held across
the await.

So a round trip is split across two cells: send in one, read the answer in the
next.

```python
# one cell — interact with the window, then run the next
gui.pump()          # dispatches what has arrived; the ordinary way to read back
```

Asking a running cell to *wait* raises `RoundTripInCell` instead of hanging
until the timeout, with a message saying so. `pump` is not that: it is a poll
of what already arrived, and it works.

One thing that would otherwise be caught by this does not need to be: **sending
a def**. `send` waits for `/done` on an ordinary carrier, but that wait is a
confirmation, not a barrier — the comm keeps order, so the def is at the engine
before the `/synth_new` that needs it. Here the confirmation is dropped and the
send returns immediately, so the first thing every notebook does keeps working.
What you give up is the early `/fail`: a def that failed to compile shows up as
one the server does not have.

This only bites the `page` backend's server leg and the GUI's queries. With
`native` the client reaches its server over an ordinary socket, where a reply
comes back as it always does.

## Why it is a separate package

`clausters` has no IPython in it — no display hooks, no `_repr_mimebundle_`, no
optional import of a notebook library. Everything a notebook needs is either a
seam that was already there (`GuiHost(interface=...)`, `Server(interface=...)`)
or lives in `clausters_jupyter`: the carrier over the kernel's comm, the
routing that gives each window its own cell, and formatters registered from
*outside* the classes they display, which is IPython's own mechanism for
exactly this.

The practical effect: a script, a test or a `clausters` used from a plain REPL
carries none of it, and the notebook support can move at its own pace.
