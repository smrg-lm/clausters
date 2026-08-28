---
name: examples
description: How to write a Clausters example — which of the three directories it belongs to (root examples/ for the server, clients/python/examples/ for the client and GUI, clients/web/examples/ for the browser), the form each one takes (a closed script, a `# %%` notebook, a page), and the idiom rules a generated example gets wrong (the client boots its own server, a beat is never a time.sleep, both ends of a client/server feature are shown). Consult before adding or editing any example, and when closing a user-facing feature, since an example is how that feature is manually tested.
---

# Writing a Clausters example

The examples are the **manual-test surface** — there is no separate smoke
checklist, so an example that exercises new behavior *is* the test, and nothing
in CI runs any of them. That makes two things true at once: an example must
actually run, and it must read like something the reader would have written.

## The rule: an example is written in its package's idiom

Which directory it lives in decides its form. This is not taste.

| Directory | Belongs to | Form |
| --- | --- | --- |
| `examples/` (root) | the **server** | a **closed script** — even the Python ones |
| `clients/python/examples/` | the **Python client** + the GUI | a **`# %%` notebook** |
| `clients/web/examples/` | the **web client** | a **page** (`.html`), or a **node script** (`.mjs`) for what a page cannot do |

The corpus says it plainly: of the 31 Python files in the root `examples/`, 30
end in an `if __name__ == "__main__":` guard and one has cells; on the client
side, 31 of 57 are cell-organized.

### The server's examples are closed scripts

They drive the server **from outside**, as one whole run — that is the thing
being demonstrated. So they take arguments, do their run, and finish with an
exit code. `argparse` or a positional `sys.argv` read, a `main()`, the
`__main__` guard. The reader runs it; they do not live inside it.

The non-Python ones are the same shape in their language: `bench.rs`,
`osc_ping.rs`, `stress.rs` are `cargo run --example` binaries;
`persistence.sh` and `midi_standalone.sh` are scripts.

### The Python client's examples are notebooks

The client is a **live-coding** surface, so its examples are written the way a
live-coder works: `# %%` cells (the VS Code / Jupyter convention) with
`# %% [markdown]` prose between them, stepped through with Shift+Enter. Code at
module level inside the cells — not buried in a `main()` the reader cannot
step into.

**This holds for every client example, offline ones included.** An NRT example
that renders a WAV and exits looks like a closed script and is not one: the
reader still wants to build the def in one cell, change it, and re-render in the
next without re-running the file. There is no "it only produces a file"
exception — if it lives under `clients/python/examples/`, it is a notebook.

The cells pair with the **dual-mode guard** as the last cell, so the same file
is both a script and a REPL session:

```python
# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(30.0)
    finally:
        session.close()
else:
    print("up - run(10) to drive it, session.close() to end")
```

`hasattr(sys, "ps1")` is the interactive-interpreter test: run as a script it
drives itself and tears down; imported into a REPL it leaves the objects live
and tells you what to call. For an example that just renders and stops, the
guard is smaller — the render call and the teardown inside it, the `else`
naming what to call by hand.

**An example that exists to be looked at ends when the person looking at it
says so.** The `run(30.0)` above is the shape for something that *finishes* — a
render, a sequence that plays out. A GUI example is the manual test surface for
whatever it draws, so a script run holds until the **window** is closed
(`gui.pump` in a loop over an `on_closed` flag; `patch1.py` and `text.py`
are the shape), and the seconds bound stays only for the cell run, where a
notebook wants the loop to give the prompt back. A window that times out ends
the test before the reader has finished reading it, which is the one failure a
by-eye check cannot recover from.

A few older examples end with plain top-level teardown and no guard
(`verbs.py`, `scoping.py`). That is the earlier form, not a second idiom: the
guard is what keeps the file from tearing itself down under the reader's feet
when it is imported.

### The web client's examples are pages, and its generators are node scripts

An `.html` the reader opens — that is the form, and it stays the form for
anything a page can do. The header comment carries what a module docstring
would. Remember `dist/` is git-ignored: `./build.sh` from `clients/web` before
the page will load anything.

The exception is **authoring**, and it is a real one rather than a
convenience: writing a bundle is not something a page does, because a bundle is
an *input* a static page boots with no interpreter at all. So a generator is a
**node script** (`.mjs`, the extension this package already uses for node in
`tools/`, and one a served directory cannot confuse with a page module),
sitting **beside the page it feeds** — `examples/panels/piano/make_bundle.mjs`
next to that example's `index.html`. It imports the built package by relative
path (`../../../dist/…`), and it is the pair of a Python example, never of the
page next to it.

**The page says which script to run.** A page whose bundle a generator writes
shows nothing until it has been written, so its header comment carries the
command (`node make_bundle.mjs`) and where the output lands. A reader who opens
a blank page and has to go looking is a reader the example failed.

## A generator writes into `out/`

An example that leaves a file behind — a bundle, a rendered WAV, a MIDI file, a
saved session, a data directory — writes it into **`out/`, at the root of its
own example tree**:

| Tree | Where a run leaves its files |
| --- | --- |
| `examples/` | `examples/out/` |
| `clients/python/examples/` | `clients/python/examples/out/` |
| `clients/web/examples/` | `clients/web/examples/out/` |

One ignored directory per tree (one line each in `.gitignore`), so a run leaves
one place to look in and one to delete. Never the working directory — a default
of `"envelope.wav"` scatters files wherever the reader happened to `cd`, and it
is what made the ignore file grow a hand-maintained list of names. Never beside
the source either: an example's directory holds what a person wrote.

The default resolves from `__file__`, not from the cwd, so it lands in the same
place however the example is run; an explicit path argument still overrides it.
`/tmp` remains right for a file the run **consumes and deletes itself** (an
impulse response rendered to be loaded back) — that is scratch, not a product.

## An example that exists twice is **one example in two languages**

`clients/python/examples/editors/composer.py` and
`clients/web/examples/editors/composer.html` are not two examples about the same topic:
they are the same example, and the pair carries the non-divergence rule the way
the clients do. So the second one is **ported, not re-invented**:

- **Same composition, same material, same names.** The lanes, the defs, the
  notes, the buffers, the labels a reader sees on screen. A lane called
  `bassline` in one is called `bassline` in the other; a take bounced offline
  from a two-beat note is bounced offline from a two-beat note, not filled with
  a wavetable because that was quicker to write.
- **Same API calls, in the same order.** Both clients have the same surface, so
  the port is a transliteration: `Sequence(Pbind(...), name="bassline")`
  becomes `new form.Sequence(new seq.Pbind(...), null, null, { name: "bassline" })`
  and nothing else changes. Where the two genuinely differ — a page has no
  filesystem, a script has no `<canvas>`, one awaits what the other calls — the
  difference is **named in the prose**, and it is the only difference there is.
- **Same prose, same order.** The docstring and the page's note explain the
  same things in the same sequence, because they are explaining the same
  example. Two texts that drifted are two readers with two mental models.
- **A screenshot of the two, side by side, should differ only where the hosts
  differ** (a window frame, a page's chrome). That is the check worth doing when
  a port lands, and it is how a whole class of "the port is not the example"
  was caught: different lane names, a wavetable where a take belonged, a
  primitive swapped for a simpler one.

What this rule is *for*: an example is the manual test surface, so a pair that
diverges silently stops testing the same thing — and the client that is not the
reference stops being checked at all.

## The idiom rules

These are what a generated example gets wrong, every time.

1. **The client boots its own server.** `Session.live()` starts one if none
   answers, and stops the one it started. An example that opens with "start a
   server first: `cargo run --release`" is documenting a limitation that does
   not exist. The one exception is an example whose *subject* is several
   independent clients against one shared server — there the boot is part of
   what is being shown.
2. **A beat is never a `time.sleep`.** A stream of events is a routine yielding
   beats on the session's clock, or a pattern. A `for` loop of `send_msg` +
   `time.sleep(gap)` is hand-rolled scheduling on the main thread: no logical
   time, no timetags, and a `TempoClock` sitting there doing nothing. Where
   `time.sleep` *is* right: the main thread waiting while the clock thread
   plays, between actions (`timeline.py`). Inside a routine it is a
   defect — the routine runs *on* the clock thread and would freeze the
   timeline.
3. **Show both ends of a client/server feature.** If a server-side action has a
   client-side counterpart — a `transport_stop` that wants a
   `TempoClock.freeze` beside it — the example does both. One end is half the
   feature taught as if it were the whole one.
4. **Reach for the API, not the wire.** `Event(...)`, `Synth(...)`,
   `node.set(...)`, `buf.gen(...)` — a raw `send_msg("/synth_new", ...)` in a
   *client* example teaches the protocol where the client's own surface exists.
   The root `examples/` are the opposite case: several of them are there
   precisely to show the wire (`json_client.py`, `shm_client.py`,
   `tcp_client.py`, `osc_ping.rs`).
5. **Nothing that is already the default is passed.** Both clients resolve an
   ambient server and clock the same way — the running routine's, else the
   **active session's**, else the default session's (`main`, which is a session
   too: that is what "no session" means, not "no environment"). So an example
   built around one session **activates it** and then names nothing it owns:

   ```python
   session = Session.nrt(tempo=2.0).activate()
   lead().send()                       # not .send(session.server)
   note = Synth("lead", {"freq": 220.0})
   Routine(sequence).play()            # not .play(session.clock)
   ```

   ```js
   const session = (await Session.nrt({ tempo: 2.0 })).activate();
   await lead().send();
   const note = new Synth("lead", { freq: 220.0 });
   new Routine(sequence).play();
   ```

   The argument comes back only where it is **not** the default: a second
   server, another session's clock, a handle the example is contrasting. A
   `session.clock` handed to a `play` on that same session is noise, and it
   teaches a reader that the wiring is theirs to carry.

   Three things this does not mean. An example with **no** session is right as
   it is — that is the common case, and the default server and clock are
   `main`'s. A **method** on the server (`session.server.sendBundle(...)`,
   `server.transportPlay()`) is not an argument and stays. And a local
   `server = session.server` is worth keeping where the example calls methods
   on it repeatedly — what goes is `server=server` on a constructor that would
   have resolved it anyway.

   **A clock made by hand is never the default — a booted server is.** The two
   are asymmetric on purpose, in both clients: `Server().boot()` claims
   `main.server` first-wins, so a later `Synth("beep")` finds it; a
   `TempoClock(2.0)` claims nothing. `main.defaultClock` stays `null` until an
   ambient `play` needs one, and then the client **builds its own** at tempo
   1.0 and starts it. So an example with no session that makes its own clock
   **passes it**:

   ```python
   clock = TempoClock(2.0, timebase=server.sample_timebase())
   Routine(sequence).play(clock)       # without it: a new clock at 1.0
   ```

   Leaving it out there is not tidier, it is a different piece — the routine
   runs on a clock the example never configured, at the wrong tempo and off
   the server's sample counter. The rule above only drops a clock that a
   session already made ambient. (A server is a process resource and there is
   normally one; a clock is cheap and a piece usually has several, which is
   why only the server adopts.) **Check this before relying on it**: it is
   what the clients do today, not a decision anything guards — a first
   hand-made clock could yet claim the default slot the way a booted server
   does. `main.get_default_clock` / `Main.getDefaultClock` is the whole of it,
   in five lines each.

   `activate()` has a counterpart: **a page that opens a second environment
   `deactivate()`s the first.** A page that renders an offline take and then
   sounds it on a live server must not leave the score session ambient, or the
   ambient `play` reaches the score instead of the engine — silence, with
   nothing to read. The scripts do not need this because they end.

6. **An example that wants to show a signal opens a host view.** `plot(...)`,
   `scope(bus)`, a `waveform`/`scope`/`spectrum` widget in a GuiDef — never a
   canvas the example feeds, and never arithmetic over samples to fill one.
   Everything drawn is drawn by the GUI host; a client names what to look at.
   The temptation lives here more than anywhere else — a page has a `<canvas>`
   in reach and a notebook has matplotlib — and it is how a scaffold becomes a
   norm: the web client once grew a whole pixel-column surface because one
   example needed it (`clients/web/PLAN.md`, W26). If the view you want cannot
   be named, that is a missing widget, decided on its own merits and in both
   clients — not something an example invents.

## The docstring is the documentation

An example documents itself and **no book page enumerates the examples** — they
travel with the repository, not with the wheel or the npm package, so a catalog
in a book serves a reader who does not have them. The module docstring (or a
page's header comment) says:

- **what it shows** — the point of interest, named with the API's own verbs;
- **what it needs** — a display, a GPU adapter, libfaust, an installed package;
- **how to run it** — the literal command, and for a notebook the "install once,
  then step through it or run it as a script" preamble.

A book's `examples.md` says only where the directories are and how to run each
family; a topic page may point at *one* example that shows what it explains.

## Before you call it done

- **Run it.** Nothing in CI does. A signature change breaks examples at a call
  site no build ever reaches.
- **Refresh the binaries first** for anything launched through Python
  (`scripts/refresh-bin.sh`) — the package is installed editable, so the copy
  bundled in `clients/python/clausters/_bin` wins over `target/` and goes stale
  the moment a crate is rebuilt. A manual test against pre-change binaries
  proves nothing.
- **Check by ear or by eye** what the example claims is audible or visible. The
  printed output is not the test; it is the narration.
