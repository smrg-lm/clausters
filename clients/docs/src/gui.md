# The visual elements: a GUI the {{alt script || page}} builds

A GUI in Clausters is a **def**, like an instrument. You build a tree of nodes,
send it in one message, and from then on you mutate live widgets one at a time.
Nothing is compiled, nothing is subclassed, and the window belongs to the **GUI
host** — <!-- python -->a separate process that owns the pixels and talks OSC<!-- /python --><!-- web -->the same host the desktop runs, compiled to wasm and drawing on a `<canvas>` in your document<!-- /web -->.

This page builds one from nothing and then walks every element there is,
finishing with an instrument that keeps its GUI after the {{alt script || page's code}} is gone.

**The host is what draws, always.** A {{alt script || page}} names what to look at — `plot(...)`,
`scope(bus)`, a `waveform`/`scope`/`spectrum` widget in a tree like the ones
below — and the host reads the buffer or the bus itself and paints it. The
client computes no picture: not a pixel column, not a trigger, not a decibel
curve. That is what keeps the Python and TypeScript clients one client in two
languages, and what keeps a figure from being drawn two ways. If you would
rather do your own arithmetic over your own canvas, the [data paths](data.md) are open and
nothing stops you — but that is your program, and it is not a surface this
project provides, documents or keeps in step.<!-- web --> In a page that is worth saying twice, because a page has a `<canvas>` of its own and the temptation is right there.<!-- /web -->

<!-- python -->
Install once, from the repo root:

```sh
python -m venv .venv
.venv/bin/pip install -e ./clients/python      # bundles the GUI binary
```

Everything below needs a display and a GPU adapter.
<!-- /python -->
<!-- web -->
Everything below needs a page served in a secure context (`localhost` counts) by a browser with WebGPU.
<!-- /web -->

## A window of controls

Start the host and open a window. {{alt `GuiHost().boot()` launches a `clausters-gui` process and connects to it || `new GuiHost().boot()` brings up the wasm host in this page}}; no audio server is involved yet.

```python
from clausters.gui import GuiHost, button, choice, knob, label, panel, slider, view

gui = GuiHost().boot()

v = view(
    label("a filter", h=24),
    panel(knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0),
          knob(name="res", label="res", min=0.0, max=1.0, value=0.3),
          layout="row"),
    panel(slider(name="mix", label="mix", min=0.0, max=1.0, value=0.5),
          button(name="reset", label="reset"),
          choice(name="wave", options=["sine", "saw", "square"], index=0),
          layout="row", h=40),
    title="filter", w=520, h=260, layout="col")

win = v.open()
```

```ts
import { loadCore } from "clausters/page";
import { button, choice, GuiHost, knob, label, panel, slider, view } from "clausters/gui";

await loadCore();
const host = await new GuiHost().boot();

const v = view(
    { title: "filter", w: 520, h: 260, layout: "col" },
    label("a filter", { h: 24 }),
    panel({ layout: "row" },
          knob({ name: "cutoff", label: "cutoff", min: 20.0, max: 20000.0, value: 800.0 }),
          knob({ name: "res", label: "res", min: 0.0, max: 1.0, value: 0.3 })),
    panel({ layout: "row", h: 40 },
          slider({ name: "mix", label: "mix", min: 0.0, max: 1.0, value: 0.5 }),
          button({ name: "reset", label: "reset" }),
          choice(["sine", "saw", "square"], { name: "wave", index: 0 })));

const win = await v.open();
```

A window appears. Four things about that tree are worth noticing before
anything else.<!-- web --> And one that is only the page's: the builders take **one options object in camelCase**, and the children come after it — `textSize` becomes `text_size` on the wire, `baseBucket` becomes the wire's base_bucket; inside an `axes` pair the keys are the wire's own (`{{wire sample_rate}}`, `sel_start`), since that object goes through untouched. Both emit the same JSON the Python builders emit, vector for vector.<!-- /web -->

**A view with no parent is a window.** There is one container, `view`, not a
window type and a panel type: nested in another view it is a panel, and at the
root it is the window. Any node opens, so the frame is only there when you want
its properties:

```python
view(knob(name="freq"), title="voice", w=200, h=200).open()   # a titled window
layout(knob(name="freq"), knob(name="res")).open()            # a window of two knobs
knob(name="freq").open()                                      # a window that is a knob
```

```ts
await view({ title: "voice", w: 200, h: 200 }, knob({ name: "freq" })).open();   // a titled window
await gui.layout({}, knob({ name: "freq" }), knob({ name: "res" })).open();      // a window of two knobs
await knob({ name: "freq" }).open();                                             // a window that is a knob
```

The last two are framed for you, in a window that hugs what it holds — the wire
opens {{alt an OS window || a canvas}} for a `window`-rooted document and nothing else. Reach for
`view()` when a title, a size or a theme matters, since those belong to a root
nobody frames. (`window` is the older spelling of `view` and still works.)

**The view opens itself.** A builder returns a `View` — the GUI's counterpart
of a `SynthDef`: a tree you compose and then send, not a live widget. So the
tree is the subject of the sentence (`view(...).open()`), the way a def is
(`synthdef.send(server)`), and `open()` finds the host the way `plot` and
`scope` do: the one {{alt `GuiHost().boot()`, `GuiHost().attach()` || `boot()`, `attach()`}} or
{{alt `Session.gui()` || `session.gui()`}} registered, else one it boots. Pass {{alt `host=` || `{ host }`}} to say which<!-- python -->, and
`gui.open(tree)` still works — it is the low-level door `open()` goes through<!-- /python -->.
(`attach()` is the host this handle did not start; see
[Sessions](sessions.md#several-servers-and-the-one-you-did-not-start).)<!-- web --> A `View`'s own properties are the JSON, so `JSON.stringify` writes what it always wrote; what is added is the name index (`v.find("cutoff")`, `v.names()`) and `open`.<!-- /web -->

<!-- web -->
**The element goes in `open`, and a view that names none gets a canvas of its own.** `view(...).open(el)` mounts the view into that element's box — the canvas inside it is made and fitted for you, following the element's size and the page's `devicePixelRatio` — and `view(...).open()` appends a canvas to the document. That is how *a view with no parent is a window* finishes in a page: **a view with no element is a canvas**, so a document holds as many as it opens, and `win.canvas` is the one this window draws on. There is no window manager: CSS sizes the canvas, the document places it, and closing the window releases it. Sizes stay logical pixels, so a tree written for the desktop comes up the same size in a tab. A host reached over a socket has windows of its own and refuses an element. That substitution is what makes a bundle mountable in the flow of a page: see [Components](components.md). A def fed straight through the binding surface, with nobody having said where, draws on the page's **fallback** canvas — appended the first time that happens, so a document whose views all name their own place never carries an empty one.
<!-- /web -->

**No widget has an id.** You passed `name` instead, and `open` hands back a
window handle you index by name — {{alt `win["cutoff"]` || `win.widget("cutoff")`}}. The host does use integer
ids on the wire, and `open` allocated them for you — **in the document it
sends**, not in the tree you wrote, because an id names a *live* widget and the
view is a definition. That is what lets one view open twice:

```python
a = v.open()
b = v.open()              # a second window; its own ids, the same view
a["cutoff"].set(value=200.0)   # b is not touched
```

```ts
const a = await v.open();
const b = await v.open();       // a second window; its own ids, the same view
a.widget("cutoff").set({ value: 200.0 });   // b is not touched
```

A name is client-side only and never travels. Unlike an id (which recycles when
a window is freed), a name is stable, which is what a live edit addresses
against.

**The children came first.** {{alt The positional slot of every builder belongs to
what the widget is made of — a container's children, a label's text, a choice's
options — || A container's children follow its options, and a label's text and a choice's options are its first argument,}} so an ordinary tree mentions no ids at all.

### Every window in one: a dock

A host gives each window {{alt a window of the platform's || a canvas}} of its own. Booted with a **dock** it draws them all in one, and every window is a **pane** of it, side by side: the panes share the width, and a pane opened later makes room for itself.

```python
gui = GuiHost().boot(dock=True)
```

```ts
const host = await new GuiHost().boot({ dock: element });
```

{{alt The dock is a window the host opens for it || The dock is the canvas inside `element`, which follows the element's box and the page's `devicePixelRatio` as a view opened into an element does}}. A pane is a window in everything but its frame: it keeps its def, its handle, its events and its picture, a press or a key reaches the pane it lands in, and closing a pane gives its room to the others. So a script written for separate windows runs unchanged in a dock<!-- web -->, and a view opened with an element draws in its pane rather than in that element<!-- /web -->. `panels/dock` shows it.

**Panels, tabs and splits.** Every pane is a panel with a bar along its top: a grip, its window's title, a button for the panel's menu and a close mark. Drag a panel by its grip onto another — into its middle to put the two in one panel, near one of its edges to stand beside it, the place it would land shown while it moves — and drag the line between two panels to give one of them more room. A panel holding several windows shows a tab for each, with a close mark each; a press on a tab shows its window, a tab dragged moves that window alone, and a panel or a tab dragged out past the dock's edge gives its window {{alt a window of the platform's || a canvas}} of its own. The panel's menu (its button, or the secondary button on its bar) lists its tabs, splits a window right or down, moves it to a page or to its own window, and closes it. Every window the host opens is a pane, an application's and the ones its gestures open included. A pane behind another's tab is not drawn and counts as not seen, as a covered window does, and keeps playing whatever it plays. The same moves are verbs a script calls, and the arrangement they leave is data it reads and puts back:

```python
gui.dock_call({"verb": "join", "pane": note, "of": tone})
gui.dock_call({"verb": "split", "pane": level, "side": "bottom", "of": tone})
layout = gui.dock_layout()
gui.dock_restore(layout)
gui.dock_capture("dock.png")
```

```ts
await host.dockCall({ verb: "join", pane: note.id, of: tone.id });
await host.dockCall({ verb: "split", pane: level.id, side: "bottom", of: tone.id });
const layout = await host.dockLayout();
await host.dockRestore(layout);
await host.dockCapture("dock.png");
```

`{"verb": "undock", "pane": ...}` takes a pane out the same way, and `{"verb": "dock", "pane": ...}` puts it back.

**Pages.** Along the dock's bottom is a strip of pages, each an arrangement of its own: a window opened with {{alt `dock_page` || `dockPage`}} names the page it opens on, which is made and shown, and every other window opens on the page shown; an application's window opens on its own page (the multitrack's, the audio editor's). A press on a page's tab shows it, the last tab makes a copy of the page shown, and a pane's tab dropped on a page's tab moves the pane there. Switching a page moves panes and frees none — a window on another page keeps playing. The verbs are `page` (show one by `name`, made if new), `place` and `leave` (a pane onto or off the page shown), `page_rename` and `page_close`, and {{api dock_layout}} and {{api dock_restore}} read and put back the page shown — with `{"pages": ..., "page": ...}`, every page.

Every call answers the dock as it stands: the layout, and where each pane, its tab and each divider are, in the pixels a capture of the dock has — what a script points at with `{"verb": "pointer", ...}` to play a drag on the tabs as a hand would.

### A control widget is built from the control it drives

A `knob`, `slider`, `number`, `toggle` or `button` takes a **def's control**
as its first argument, and reads its name and its default off it:

```python
freq = control("freq", 220.0)
sd = SynthDef("voice", out(0.0, sine(freq=freq) * 0.2))

knob(freq, min=110.0, max=880.0)             # the control object you held
slider(sd["amp"], min=0.0, max=1.0, label="level")   # or one off the def
view(*[knob(c, min=0.0, max=1.0) for c in sd.controls])
```

```ts
const freq = control("freq", 220.0);
const sd = new SynthDef("voice", out(0.0, sine(freq).mul(0.2)));

knob(freq, { min: 110.0, max: 880.0 });                     // the control object you held
slider(sd.control("amp"), { min: 0.0, max: 1.0, label: "level" });   // or one off the def
view({}, ...sd.controlNames().map((n) => knob(sd.control(n), { min: 0.0, max: 1.0 })));
```

**The range is the control's when its def declares one, and the widget's
otherwise.** A **Faust** parameter declares it in the DSP, in the `hslider` that
made it; a SynthDef control may declare it where the control is declared —
{{alt `control("freq", 440.0, min=20.0, max=20000.0, warp="exp", unit="Hz")` || `control("freq", 440.0, { min: 20.0, max: 20000.0, warp: "exp", unit: "Hz" })`}} — and a
widget built from either starts from it. A control with no range says so rather
than being drawn over a guess, and {{alt a keyword || an option}} still wins over what the control
says: the control says what it is, the call says how to draw it. A declared
range is about drawing only: nothing on the server holds a `/node_set` to it.

All three def families answer the same way — {{alt `sd["freq"]`, `fd["cutoff"]`,
`gd["mix"]` || `sd.control("freq")`, `fd.control("cutoff")`, `gd.control("mix")`}} — so the widget does not care which built it.<!-- web --> (A class here cannot take a bracket without an index signature over everything else on it, which is why it is a method where Python indexes.)<!-- /web -->

And the range is **linear** unless the widget says otherwise. Two {{alt keywords || options}} say
otherwise, and a control that wants both spells both:

```python
slider(sd["amp"], min=0.0, max=1.0, curve=4.0)        # fine at the bottom
number(label="note", min=0.0, max=127.0, step=1.0)   # whole note numbers
```

```ts
slider(sd.control("amp"), { min: 0.0, max: 1.0, curve: 4.0 });   // fine at the bottom
gui.number({ label: "note", min: 0.0, max: 127.0, step: 1.0 });   // whole note numbers
```

{{alt `warp="exp"` || `warp: "exp"`}} makes the travel **ratios** instead — halfway between 20 and
20000 is 632 — over a positive range, and wins over `curve`.

`curve` bends the range the handle travels: `0` (the default) is linear,
negative spends most of the range on the first half of the travel and positive
on the last half — the fine-at-the-bottom feel a frequency or an amplitude
control wants. It is the same bend `lincurve` runs and an envelope
segment runs on the audio thread, because the host reads it out of the shared
core rather than deriving one of its own: a control feels the way the value it
produces was computed.

`step` is the grid a **drag** lands on, in the value's own units. It is counted
from `min` and never leaves the range, so a grid that does not divide it
(`0..10` by `3`) stops at `9` rather than on an off-grid `10`. A Faust
parameter arrives with the step its `hslider` declared, like its range. A value
*you* send — {{alt `value=`, or `win.widget("note").set(value=...)` || `value`, or `win.widget("note").set({ value })`}} — is drawn as sent: the
step is a rule about the hand, not a constraint on what the {{alt script || page}} may say.

There is deliberately no *named* spec ({{alt `spec="freq"` || a `spec: "freq"`}} for 20..20000
exponential) for a widget you build. These {{alt keywords || options}} are what one would be built
out of, and a name that silently drew the wrong curve would be worse than no
name at all. The one place a control's name stands in for its range is a node's
generic editor ([`edit(node)`](document.md#a-running-node-editnode)), which has
to draw every control of a def whether it declares a range or not.

The widget's `name` becomes the control's name, which is what the handle
addresses it by, and what [binding](#values-that-never-come-back-to-the-script) uses to
reach the synth.

### The two switches, and what a press means

A `button` is momentary and a `toggle` latches, and both send a **pair of
values** rather than a boolean: `on` and `off`, `1`/`0` unless you name another
pair. A bypass lives at `0.0`/`0.7` and a mode at `1`/`2`, and neither is a span
a widget could be drawn over — which is why it is a pair and not a `min`/`max`:

```python
toggle(sd["bypass"], on=0.7, off=0.0, label="wet")
```

```ts
gui.toggle(sd.control("bypass"), { on: 0.7, off: 0.0, label: "wet" });
```

A button's `mode` says which of the two pointer primitives reaches the server:

```python
gate = control("gate", 0.0)
fire = control("fire", 0.0, rate="tr")

button(gate, label="hold")                  # `on` while held, `off` on release
button(fire, mode="press", label="fire")    # one message, the bang
```

```ts
const gate = control("gate", 0.0);
const fire = control("fire", 0.0, { rate: "tr" });

button(gate, { label: "hold" });                  // `on` while held, `off` on release
button(fire, { mode: "press", label: "fire" });   // one message, the bang
```

- `"gate"` (the default) sends `on` at the press and `off` when the button is
  let go, so the value lasts exactly as long as the button is held — what an
  `{{api env_gen}}` gate reads, and what a trigger control ignores the tail of by
  definition.
- `"press"` sends `on` at the press and **nothing** after it.

**A widget cannot make a value instantaneous.** What is sent is held by whoever
receives it, so `"press"` is a bang only against something that returns to zero
on its own: a trigger control ({{alt `rate="tr"` || `rate: "tr"`}}), which the server resets after one
block. Over any other control it would leave `on` standing forever, and the
client refuses to build that pair rather than letting you find it by ear. A
button driving no control has no such trouble — it emits a `/gui_event` and one
message *is* an event.

**Press and release are the primitives, and a click is not a mode.** Everything
else a pointer does to a button is composed from the two: a click is a press and
a release that landed inside, a double click is two of those inside a window.
Those are gestures, and what a `mode` says is only which primitive reaches the
server.

### What the hand did, as against what the widget is worth

A button says two things at once, to two audiences. Its **value** is a control
signal — `on`/`off`, which a binding forwards to the audio server without this
{{alt script || page}} ever seeing it. Its **interface events** are what the hand did, and they
have three verbs of their own:

```python
win["play"].on_click(start)      # the press was completed on the button
win["key"].on_press(sound)       # the pointer went down
win["key"].on_release(silence)   # ...and came up, wherever it came up
```

```ts
win.widget("play").onClick(start);      // the press was completed on the button
win.widget("key").onPress(sound);       // the pointer went down
win.widget("key").onRelease(silence);   // ...and came up, wherever it came up
```

`{{api on_click}}` is what a command button wants: press, slide off the button,
release, and **nothing happens** — the cancellation every desktop convention
gives an "Accept", and the one a piano key must not have. `{{api on_press}}` is the
other side of that: the note is at the down-stroke, no second thoughts.
`{{api on_release}}` fires for an abandoned press too, which is exactly what makes it
the release and not the click.

They reach the {{alt script || page}} **whether or not the widget is bound**. A binding forwards
a widget's *value*, and a command is not a value, so one button can drive a
synth's gate and run a {{alt script's || page's}} action at once:

```python
win.bind(synth)                            # the gate goes straight to the server
win["hold"].on_click(lambda: print("!"))   # and the click still arrives here
```

```ts
win.bind(synth);                                 // the gate goes straight to the server
win.widget("hold").onClick(() => console.log("!"));   // and the click still arrives here
```

`{{api on_event}}` is the raw stream and sees everything, these three included (they
arrive as the one-string payload they are), so reading the value and the
command on one widget is two registrations and no filtering. All four survive a
redraw: a callback belongs to the widget the *name* points at, not to the id it
happened to have.

### Driving it, and listening

Setting a prop is one call, and it is live:

```python
win["cutoff"].set(value=2000.0, label="freq")
```

```ts
win.widget("cutoff").set({ value: 2000.0, label: "freq" });
```

Reading the user's side is a responder, in the same shape the audio client uses
for OSC:

```python
win["cutoff"].on_event(lambda v: print("cutoff ->", v))
win["reset"].on_event(lambda v: print("reset", v))
```

```ts
win.widget("cutoff").onEvent((v) => console.log("cutoff ->", v));
win.widget("reset").onEvent((v) => console.log("reset", v));
const info = await win.widget("cutoff").query();   // a round trip, so awaited
```

Turn the knob and click the button: the values print. Nothing in the {{alt script || page}}
delivers them — an open window is delivered to.

### The event loop, and the one call a script ends with

<!-- python -->
Opening a window starts the host's **event loop**: a thread of its own that
drains the socket and calls the callbacks as the messages arrive. An open window
is the thing that *has* events, so having one is what makes the loop necessary,
and there is nothing to switch on:

```python
gui.looping               # True from the first open window
gui.loop                  # the loop itself, if you want to schedule on it
```

What a script still has to do is **stay alive** while a hand uses the window,
and that is one call:
<!-- /python -->
<!-- web -->
**Nothing is pumped.** A page already has an event loop, so a handler fires when the message arrives and there is no loop of the client's to start. Building is synchronous; opening is awaited (the host may still be booting), as is anything that waits for the host to *answer*. A page never exits and must never block, so **waiting for a window to close** answers a promise — not how a program stays alive, but how one sequences on the close:
<!-- /web -->

```python
win.wait()                # until this window is closed
gui.wait()                # until every window this host opened is closed
win.wait(10.0)            # or bounded: False if it is still open after ten seconds
```

```ts
await win.wait();         // until this window is closed
await host.wait();        // until every window this host opened is closed
await win.wait(10);       // or bounded: false if it is still open after ten seconds
```

<!-- python -->
**A notebook calls neither.** The loop runs whether or not anyone waits, so a
`# %%` cell that opens a window is finished when it returns and the next cell
edits the window that is already there. Waiting is what a script does *instead
of exiting*, and a cell has nothing to exit from. That is the only difference
between the two ways of running the same file.
<!-- /python -->

The same verb is on everything that opens a window — `plot(...)`, `scope(...)`,
`{{api plot_def}}(...)`, `edit(x)` — with `closed` beside it for the same question asked
without {{alt blocking || waiting}}.

<!-- python -->
`pump` and `poll` are still there, and from the first open window they stand
down: `pump` answers `0` and `poll` answers `None`. Two drains over one socket
would race for every message and deliver half of them twice. A script written
around `poll` therefore keeps running beside the loop; it simply stops being the
thing that delivers. To keep draining by hand instead, build the window with
`define` rather than `open` — nothing this client remembers is opened, and no
loop starts.
<!-- /python -->

A **reply is not an event**, and the loop keeps them apart: `query`'s answer
goes to whoever asked rather than to the callbacks. That is also the only
correct shape for it — reading the {{alt socket || carrier}} by hand takes whatever arrived next,
so an event landing between the question and the answer loses the reply.

## What a `type` names

Every node is `{id, type, props, children}`, and a `type` names one of three
things:

| Kind | What it is | Types |
|---|---|---|
| **Container** | owns 0, 1 or 2 **axes**, and so a coordinate system its children are placed in | `window`, `layout`, `plane`, `field` |
| **Element** | draws against the axes of the container holding it | `signal`, `notes`, `curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label`, `separator`, `progress`, `post` |
| **Control** | an element with a value and no axis | `slider`, `knob`, `number`, `button`, `toggle`, `text`, `choice` |

That is the whole vocabulary. The builders you just used — `panel`, and later
`waveform`, `timeruler`, `scope` — are **shortcuts**: each builds one of those
nodes with the props of one common case. `layout`, `plane` and `signal` are
there beside them for the cases no shortcut names, and you can mix the two
freely in one tree. A `field` is only a free-standing ruler, which `timeruler`
writes.

Two consequences worth having in mind while reading on. A **ruler, a navigation
window, a selection, a playhead and a value range belong to a container's
axes**, not to each element drawn against them — which is why several views can
share one time axis without any of them owning it. And an **element's
capabilities are props**: whether a view navigates, carries a selection or
edits back is a choice over any presentation, not a different kind of widget.

From here on the snippets are **trees**, not whole {{alt scripts || programs}}: each one is
something you `open()` on the host you already booted, wrapped in `view(...)`
when the window's own properties matter.

## The keyboard, and where it points

One widget at a time holds the **focus**, and it is the only one keys reach.
Click a `text` field to focus it and type; press **Tab** to walk the window's
focusable widgets in the order they are laid out, **Shift+Tab** back along
them -- except out of a `code` editor, which keeps Tab to indent with: from it
the ring moves on **Ctrl+Tab** and **Ctrl+Shift+Tab**. The focused widget wears a ring in the theme's `focus` role, so where the
keyboard is pointing is always visible.<!-- web --> A canvas is focusable, and while it holds the focus the host reads the keys.<!-- /web -->

**Every control is a stop on that ring**, worked by the keys a reader already
knows: Space or Enter is a click on a `button` and flips a `toggle`; the arrows
step a `slider`, a `knob` and a `number` — by their own `step`, ten at a time
with Shift — and Home and End take them to their ends; the arrows move a
`choice`, and Enter opens its list.

A {{alt script || page}} can point it too — for a field that should be ready to type into the
moment its window opens:

```python
win["name"].focus()             # ...and .focus(False) gives it up
win["name"].on_event(lambda *a: print(a))
# ('focus', 1) when it gains the keyboard, ('focus', 0) when it loses it
```

```ts
win.widget("name").focus();     // ...and .focus(false) gives it up
win.widget("name").onEvent((...a) => console.log(a));
// ["focus", 1] when it gains the keyboard, ["focus", 0] when it loses it
```

Both ends of every move **the user makes** are reported, so a {{alt script || page}} that mirrors
the focus hears about the widget that lost it as well as the one that gained it.
The light controls are the exception: a `button`, a `toggle`, a `slider`, a
`knob`, a `number` and a `choice` take the focus without saying so, because what
they report is their value — to a callback of one argument, which a `("focus", 1)`
would not fit.
Your own `focus()` is not echoed back — no `set` is, since you already know what
you asked for. A widget that reads no keyboard refuses the focus rather than
swallowing it.

**Tab past the last widget hands the keyboard back.** A window in a web page
sits inside a document, and a ring that wrapped would trap the keyboard in the
canvas and leave the page around it unreachable — so the ring runs out, the canvas blurs, and the
browser's own tab order carries on. On the desktop nothing is focused and the
next Tab enters the ring again.<!-- web -->

Composition (IME) and the system clipboard stay the **page's**: a canvas cannot host an input method, so the host reads the keys it is handed and no more, and the clipboard a field cuts and pastes through is its own, page-wide. Text that needs composing is not entered through a host field today.<!-- /web -->

## Arranging: `layout` and its flows

A `layout` has no axes; it arranges its children by `flow`:

```python
from clausters.gui import layout

layout(a, b, c, flow="row")               # side by side
layout(a, b, c, flow="col")               # stacked (the default)
layout(a, b, c, flow="grid", cols=2)      # a fixed grid
layout(a, b, flow="free")                 # x/y place the children absolutely
```

```ts
import { layout } from "clausters/gui";

layout({ flow: "row" }, a, b, c);         // side by side
layout({ flow: "col" }, a, b, c);         // stacked (the default)
layout({ flow: "grid", cols: 2 }, a, b, c);   // a fixed grid
layout({ flow: "free" }, a, b);           // x/y place the children absolutely
```

`panel(...)` is the same node — it takes `layout` as the old spelling of
`flow`, which is why the first window read {{alt `layout="row"` || `layout: "row"`}}.

Every widget, whatever its type, may carry the **place props**: `w`, `h`,
`weight`, and `x`/`y` inside a `free` parent. On a row's or a column's main
axis the size resolves in one order — an explicit size wins, then an explicit
`weight` takes that share of the leftover, then the widget's **natural size**
(how big that kind wants to be), then an equal share. So a column of controls
is a stack of control-high rows, and a `weight` of 1 is what stretches something
past its natural size.

**No widget is ever placed smaller than its floor** — the smallest size it
still reads at: a control's box one line high, a caption cut to an ellipsis,
a knob's disc and its number, a slider's grip and some travel, a number's
widest value. A container's floor is its children's, so a row declared
28 high that holds a knob is as tall as the knob needs, and a window's floor is
the smallest the window can be made. Across a row, and in a grid cell, a
widget fills the room it is given; `{{api max_w}}`/`{{api max_h}}` keep it smaller and
`anchor` (`"center"`, `"top_left"`, …) says where it stands, while
`{{api min_w}}`/`{{api min_h}}` raise its floor. {{alt `panels/palette` || `panels/palette.html`}} shows every standard widget
at its defaults in each layout:

```python
view(
    panel(label("transport"), h=28),      # chrome, at the height it names
    waveform(name="take", data=take, weight=1.0),   # the work surface takes the rest
    layout="col", margin=8.0, gap=6.0)
```

```ts
view({ layout: "col", margin: 8.0, gap: 6.0 },
     panel({ h: 28 }, label("transport")),                    // chrome, at the height it names
     gui.waveform({ name: "take", data: take, weight: 1.0 }));  // the work surface takes the rest
```

A container has no natural size of its own — it takes what it is given —
unless it carries **`hug`**, and then it wants exactly what it holds: a row
adds its children up along its axis and takes the tallest of them across it, a
column the other way round, a grid counts its cells. That is the strip above
without the number:

```python
view(
    panel(label("transport"), button(label="play"), layout="row", hug=True),
    waveform(name="take", data=take, weight=1.0),
    layout="col", margin=8.0, gap=6.0)
```

```ts
view({ layout: "col", margin: 8.0, gap: 6.0 },
     panel({ layout: "row", hug: true }, label("transport"), button({ label: "play" })),
     gui.waveform({ name: "take", data: take, weight: 1.0 }));
```

`hug` is asked of the whole subtree, so a plain panel nested inside a hugging
one is measured too, and an axis a child leaves elastic — a plane, a multitrack, a
heavy view — is one the container hands back to the layout. On the `window`
itself it sizes the window: a window holding one knob opens knob-sized instead
of putting a strip at the top of an empty pane.

What a size may read is fixed by **where the value is resolved**. A prop that
settles when you build it or `set` it may size a hugging container — a label's
text, a choice's options. A **value** never sizes anything: the option a choice is
on, what a field holds, what a number reads, a view's samples. So a stream of
values cannot move a layout, and a control does not resize under the gesture
writing it.

A container additionally takes `margin` (the inset before its children), `gap`
(between them) and `cols`.

### One child at a time: tabs

{{alt `flow="stack"` || `flow: "stack"`}} shows a single child — the one `index` names — and neither lays
out nor draws the others:

```python
from clausters.gui import stack, spectrogram, waveform

stack(waveform(name="wave", data=take),
      spectrogram(data=take),
      name="pages", index=0)
```

```ts
import { spectrogram, stack, waveform } from "clausters/gui";

stack({ name: "pages", index: 0 },
      waveform({ name: "wave", data: take }),
      spectrogram({ data: take }));
```

The hidden page stays in the tree, so a heavy view keeps its GPU slot and its
bus reads across a switch and comes back without re-uploading anything. Flip it
from the {{alt script || page}} with {{alt `win["pages"].set(index=1)` || `win.widget("pages").set({ index: 1 })`}} — or, better, let a control do
it without the {{alt script || page's code}} at all. `tabs` and `pager` are that composition, built
for you: a `choice` in its `"tabs"` or `"pager"` view, bound to the `index` of
a stack.

```python
from clausters.gui import tabs

tabs(waveform(name="wave", data=take), spectrogram(data=take),
     titles=["wave", "spectrum"])
```

```ts
import { tabs } from "clausters/gui";

tabs({ titles: ["wave", "spectrum"] },
         waveform({ name: "wave", data: take }), spectrogram({ data: take }));
```

## The window's chrome: menus, tools and a dialog

A work surface is the middle of a window; the rest of it is **chrome**, and it
is composed from the same elements — a few more props, no new kind of node.<!-- web --> It is the host's here as on the desktop: a window's menu bar, a button's menu, a context menu, a tip, the list a `choice` opens and a dialog all open in the host's popup layer, over the canvas and inside it — never as DOM. The page answers the secondary button for a canvas whose tree has a `context` menu under the pointer, and a finger held still is the same request.<!-- /web -->

**A menu is a tree of entries, and a prop.** `menu` builds the tree, `entry`
one entry of it, and `"-"` is a separator. An entry is an action unless it says
more: `checked` makes it a check, `group` one of several, `menu` a submenu.
Three props carry a menu, and they are the three places one is shown:

```python
from clausters.gui import ICON, button, entry, menu, separator, toolbar, view

bar = menu(entry("File", menu=menu(entry("Open", "open"), "-",
                                   entry("Loop", "loop", checked=False))),
           entry("About", "about"))

tools = toolbar(button(name="play", icon=ICON.play, flat=True, tip="Play"),
                separator(),
                button(label="tools", menu=menu("Split", "Join")))

win = view(tools, work, menu=bar, context=menu("Copy", "Paste")).open()
win.handle().on_event(lambda tag, *payload: print(tag, payload))
# ('menu', ('open',)) for an action, ('menu', ('loop', 1)) for a check
```

```ts
import { button, entry, ICON, menu, separator, toolbar, view } from "clausters/gui";

const bar = menu(entry("File", undefined, { menu: menu(entry("Open", "open"), "-",
                                                     entry("Loop", "loop", { checked: false })) }),
                 entry("About", "about"));

const tools = toolbar({},
                      button({ name: "play", icon: ICON.play, flat: true, tip: "Play" }),
                      separator(),
                      button({ label: "tools", menu: menu("Split", "Join") }));

const win = await view({ menu: bar, context: menu("Copy", "Paste") }, tools, work).open();
win.handle().onEvent((tag, ...payload) => console.log(tag, payload));
// "menu" ["open"] for an action, "menu" ["loop", 1] for a check
```

`menu` on the `view` is the window's **menu bar**, a band the host draws along
the top the way it draws the status bar along the bottom. `menu` on a `button`
is the list that opens under it, and `context` on any widget is the menu the
secondary button opens there — the nearest ancestor that carries one answers.
What a pick reports is the entry's **verb**, from the widget that carries the
menu.

**A toolbar is a row of tools as wide as what they say.** `toolbar` is a
hugging, packed `row`: a `flat` button draws no box until the pointer is over
it, `icon` is one character — there are no icons in the host and there are
fonts, and `ICON` names the glyphs its own face draws — a `separator` splits
the groups, and one with a `weight` is the spring that sends the rest to the
far edge. `tip` on any widget is the text that shows once the pointer has
rested on it. A character the host has no face for — a symbol of a music font
— is still an icon when the window brings its outline: {{alt `view(glyphs={"E1D5": path})` || `view({ glyphs: { E1D5: path } })`}} maps a codepoint to the SVG path of its shape, and from then on that
character is drawn as the shape wherever text is.

**A container shows itself with props.** `title` and `frame` make a `panel`
a group, and `collapsed` a section that folds on its title strip. `split`
on a `row` or a `col` turns the gaps into dividers a drag moves; `bars` on
a `scroll` draws its scroll bars. Each reports what a hand changed —
`("collapsed", 1)`, `("split", sizes...)` — and keeps nothing for you: these
are props, set live and read back with `query`.

**A dialog is defined to open and freed to close.** `dialog(...)` is a `layout`
that stands over the window, centred, with everything behind it dimmed and out
of reach — of the pointer and of Tab alike. {{alt `enabled=False` || `enabled: false`}} does the same to
one widget, or to a container and everything in it.

**Rows of data and files.** `table` is a list, a table and a tree as one
element: rows of cells under `columns`, and a {{alt `row(..., depth=, open=)` || row with a `depth` and `open`}} is a
row of a tree. It reports `("select", ...)`, `("activate", i)`, `("open", i,
1|0)` and `("sort", column, direction)` — and ordering the rows is yours: the
host moves the mark and asks. `files(path)` is a file chooser the host lists
itself, from the disk in a window and from the page's storage in a tab (where `/` is its root), and
`{{api file_dialog}}` puts one in a dialog with a field and two buttons.

{{alt `examples/panels/chrome.py` || `examples/panels/chrome.html`}} holds one of each in a single window.

## Values that never come back to the script

A widget can be **bound**, and then its value bypasses {{alt this process || the page's code}} entirely.

To the **audio server** — the low-latency path, since nothing waits on {{alt Python's
scheduler || the page's thread}}. When the widgets were [built from the def's
controls](#a-control-widget-is-built-from-the-control-it-drives), each already
knows what it drives, so the whole surface is one verb:

```python
win = view(knob(freq), slider(amp)).open()
win.bind(synth)          # /node_set <synth> freq, /node_set <synth> amp
win.unbind()             # they emit /gui_event here again
```

```ts
const win = await view({}, knob(freq), slider(amp)).open();
win.bind(synth);         // /node_set <synth> freq, /node_set <synth> amp
win.unbind();            // they emit /gui_event here again
```

`win.controls` says what is there ({{alt `{"freq": "freq", "amp": "amp"}` || `{ freq: "freq", amp: "amp" }`}} — the
widget's name on the left, the control it drives on the right; they need not be
the same string). A window where nothing was built from a control refuses to
bind, since that can only be a mistake.

One at a time is the same thing spelled out, and is what you reach for when the
target is *not* a def control:

```python
win["cutoff"].bind("/node_set", 1000, "freq")
```

```ts
win.widget("cutoff").bind("/node_set", 1000, "freq");
```

Turning that knob sends `/node_set 1000 freq <value>` straight from the host.
To **another widget** — its value lands on that widget's prop:

```python
win["picker"].bind_widget(win["pages"], "index")
```

```ts
win.widget("picker").bindWidget(win.widget("pages"), "index");
```

A `choice` bound to a stack's `index` *is* a tab bar: the pages flip inside the
host, and nothing {{alt prints here || reaches the page}} while you click. A binding fires an **apply,
never another binding**, so two widgets wired to each other settle instead of
cascading.

**A binding to a node's control comes back.** The host asks the server, a few
times a second, for what each of its bindings to `/node_set` writes — with
`/synth_get`, which answers with the very `/node_set` the binding would send —
so a bound knob moves when the control changes from anywhere else: a
`synth.set(...)` from this {{alt script || page}}, another client, a plugin's own window, a
loaded state. The knob a hand is holding is left alone, so a drag is never
pulled back under the pointer. A binding to `/node_setText` comes back the same
way, as the control's value **as text** — the plugin's own words for it, or the
number with its unit — which is how a `label` bound to it shows a value it never
writes. A control mapped to a control bus reads back what the bus carries, so
its knob follows the bus; one mapped to an audio bus reads back what was last
set on it.

Both matter most for a **bundle**: a GuiDef whose widgets are bound and whose boot list starts its graph, which a page mounts with no client library running afterwards — the [last section](#the-instrument-without-the-script-a-bundle).

### A panel that keeps its own undo

A bound panel has no document behind it, and its values never pass through
{{alt this process || the page's code}}, so the {{alt script || page}} cannot keep what the hand did to it. The window
can:

```python
win = view(knob(freq), slider(amp), title="lead", history=True).open()
win.bind(synth)
```

```ts
const win = await view({ title: "lead", history: true }, knob(freq), slider(amp)).open();
win.bind(synth);
```

With {{alt `history=True` || `history: true`}} the host keeps an **undo order of its own** for that
window, and Ctrl+Z and Ctrl+Shift+Z (or the window's `undo`/`redo` verbs) walk
it there instead of reporting them to the {{alt script || page}}. Its rules:

- **A gesture is one entry**: a drag from press to release, a run of wheel
  notches or arrow keys on one control, what was typed in a field while it held
  the keys, a paste. A button is never one.
- **The target is the parameter.** Two widgets bound to one synth control are
  one target, and an undo moves both.
- **An undo writes the value the entry stored**, through the widget's binding
  (or as the event a hand would send, for an unbound control), whatever the
  parameter holds by then. A value that comes from elsewhere (a read-back, a
  `host.set`) is no entry.
- **A node that ends takes its entries out of the order**, and so does a target
  no widget of the window writes any more.
- **Each window is its own order.** Defining the window again keeps it, freeing
  it ends it, and loading a stored def starts it afresh: `history` is part of
  the def, the entries never are.

A focused `knob`, `slider` or `number` also **copies its value** (Ctrl+C) onto
the host's clipboard as text and **pastes a number** (Ctrl+V), so a value
travels between two panels, or to and from another program. Anything else on
the clipboard is refused, and the status line says what it holds.
{{alt `examples/panels/undo.py` || `examples/panels/undo.html`}} has two such panels side by side.

## The signal element: every view of a signal

Six names in the old catalog — a waveform, a plot, an oscilloscope, a
spectroscope, a spectrogram, a goniometer — were six points of one element:

```text
signal(view, <source>, navigable, selectable, editable, layers)
```

- **`view`** is the presentation: `"trace"` (value against time, the default),
  `"spectrum"` (magnitude against frequency), `"spectrogram"` (the STFT,
  magnitude against time *and* frequency), `"phase"` (the goniometer of a
  stereo pair).
- **the source** is either a `bus` (with `rate`), read forward-only, or
  addressable samples — `data`, `blob`, `buffer`, `path`, `cache` — which is
  what lets a view navigate, slice and select.
- **the capabilities** are `navigable`, `selectable`, `editable`.
- **`layers`** is the stack the picture is, back to front: `"peak"` (the
  min/max envelope the signal reached), `"rms"` (the symmetric body of the
  level it held), `"signal"` (the reconstruction between the samples),
  `"momentary"`/`"short"` (the loudness curves) and `"spectrogram"` (the
  time-frequency texture). `measure` is the same prop under its older name.

### The layer stack

The layers are a *factor* of the view, not a composition of widgets — and the
order is yours, back to front. The classic editor picture is the RMS body drawn
inside the peak envelope:

```python
from clausters.gui import waveform

waveform(cache="take.clpk", layers="peak rms", label="the take")
```

```ts
import { waveform } from "clausters/gui";

waveform({ cache: "take.clpk", layers: "peak rms", label: "the take" });
```

One view and not two, because **every view of a signal paints its own field
before it draws**: two of them on one rectangle do not layer, the second hides
the first. Drawing twice into one body is also what keeps the rest single — one
axis, one ruler, one selection, one playhead, one upload of the samples.

A layer that has something to say about itself is written as {{alt a dict || an object}} instead of a
name, and then the stack is a list:

```python
from clausters.gui import spectrogram

spectrogram(path="take.f32", sample_rate=rate, layers=[
    "spectrogram",                                  # the texture, at the bottom
    {"draw": "peak", "y": "box", "alpha": 0.6},     # the wave, read through it
    {"draw": "rms", "y": "box", "alpha": 0.4},      # and the level inside that
])
```

```ts
import { spectrogram } from "clausters/gui";

spectrogram({ path: "take.f32", sampleRate: rate, layers: [
    "spectrogram",                                  // the texture, at the bottom
    { draw: "peak", y: "box", alpha: 0.6 },         // the wave, read through it
    { draw: "rms", y: "box", alpha: 0.4 },          // and the level inside that
] });
```

- **`alpha`** is that layer's own weight over what is under it — which is what
  reading one picture *through* another is made of. It is not the widget's
  `opacity`, which fades everything including the chrome, and it is live:
  {{alt `host.set(id, layers=[…])` || `host.set(id, { layers: [...] })`}} re-states the stack between two frames.
- **`visible` and `solo`** are how you read a stack while you build it. A
  hidden layer keeps its place in the order, so showing it again puts it back
  where it was; and where anything solos, only the soloed layers are drawn.
- **`y`** is which vertical the layer is read on. `"axis"` maps through the
  body's own — the amplitude or frequency window a zoom opens — and is what the
  y ruler and the cursor read-out report; `"box"` normalizes into the rectangle
  with a scale of its own, which is what a loudness curve already does. **Two
  layers claiming the axis for different quantities is refused**: a spectrogram
  measures hertz and a wave measures amplitude, so one of the two says
  `"box"` and the ruler follows the other. Refused rather than resolved —
  whichever lost would be drawn on a scale that is not its own, which is a
  picture that lies about what it shows.

The stack is also the compositing order: the field is painted once, the texture
is sampled where the stack puts it, and the layers after it draw over it. A
view that names both reads the **samples** (inline, a mapped file, a server
buffer) and makes both pictures from them — the pyramid for the traces, the
analysis for the texture — so a source that is a summary and nothing else (a
peaks cache) draws no `"spectrogram"` layer.

Three things follow from what the level measure is:

- **The level is averaged over a fixed 50 ms of the source**, not over the pixel
  column it is drawn in — 2400 samples at 48 kHz, the RMS window an audio editor
  defaults to. A root-mean-square is an average over a *duration*, so a window
  that followed the column would make the body's own values follow the **zoom**:
  the level would move while you navigated samples that had not changed. This
  way it stands still. (A live bus view, whose window is already stated in
  milliseconds and whose rate the host does not know, averages what each column
  covers.)
- **The body disappears when the envelope comes down onto it.** The envelope
  narrows as you zoom in, since a column covers less of the wave; once it is
  within a fifth of the level the two are saying the same thing, so the layer
  goes — at full weight, in one step, rather than fading. That is also what
  keeps it from ever showing outside the envelope that contains it. It is the
  behavior an audio editor's RMS layer has: it is a reading of the overview, and
  zooming in to work leaves you the wave.
- The body costs no second pass over the samples. The mean square rides in the
  peak cache beside the min and max, at every resolution level, so the body
  reads the same mapped file the envelope does. **A cache built before the level measure existed draws no body**: its energy was
  never measured, and zeros would say silence over samples that are not
  silent — so the layer is simply absent, and rebuilding the cache is what fills it in.

A take you are **recording into** wants one more prop, `fills`:

```python
waveform(buffer=take.bufnum, sample_rate=rate, fills=True)   # ...and fills=False when it stops
```

```ts
waveform({ buffer: take.bufnum, sampleRate: rate, fills: true });   // ...and fills: false when it stops
```

It says the samples are being written as they are drawn, so the view stops at the
buffer's write frontier and leaves the axis past it empty. Without it the
buffer's own zeros are drawn — the minimum-ink rule puts a flat line across the
whole take before anything has been recorded into it — because past the frontier
there is no silence, there are no samples yet. The host cannot work this out for
itself: a frontier alone does not tell a recording from a loaded take that one
write touched, and you are the one who allocated the buffer.

The shortcuts name the six common points, and the props of each are documented
with them in the {{alt [API reference](api/clausters.gui.guidef.md) || [API reference](api/Namespace.gui.md)}}:

```python
from clausters.gui import (phasescope, plot, scope, spectrogram, spectrum, waveform)

waveform(path="take.f32", channels=2)     # signal(view="trace"), navigable
plot(data=seq)                            # signal(view="trace", navigable=0)
scope(0, channels=2, trigger=0.05)        # signal(view="trace", bus=0)
spectrum(0, freq_scale="mel")             # signal(view="spectrum", bus=0)
spectrogram(path="take.f32")              # signal(view="spectrogram")
phasescope(0)                             # signal(view="phase", bus=0)
```

```ts
import { phasescope, plot, scope, spectrogram, spectrum, waveform } from "clausters/gui";

waveform({ path: "take.f32", channels: 2 });      // signal({ view: "trace" }), navigable
plot({ data: seq });                               // signal({ view: "trace", navigable: 0 })
scope(0, { channels: 2, trigger: 0.05 });          // signal({ view: "trace", bus: 0 })
spectrum(0, { freqScale: "mel" });                 // signal({ view: "spectrum", bus: 0 })
spectrogram({ path: "take.f32" });                 // signal({ view: "spectrogram" })
phasescope(0);                                     // signal({ view: "phase", bus: 0 })
```

They are shortcuts, not the catalog: a point the six names never froze is
written by naming the point, not by finding a name for it —

```python
from clausters.gui import signal

signal(view="spectrogram", bus=0, retention=8.0, navigable=True)   # a waterfall
```

```ts
import { signal } from "clausters/gui";

signal({ view: "spectrogram", bus: 0, retention: 8.0, navigable: true });       // a waterfall
```

### What `navigable` navigates

An axis is navigable when its domain is **addressable**, and the two live axes
reach that differently:

- **time**, on a forward-only source, is not addressable at all: there is
  nothing behind the newest window to zoom out to. `retention` (in seconds) is
  the policy that supplies a past — the host keeps that many seconds of the bus
  and the view reads *that* — which is what makes the waterfall above a
  spectrogram you can zoom and pan like a file's. A live axis follows the
  newest until you navigate it, and then stays where you put it.
- **frequency** is addressable with no history at all: every bin is there every
  frame. So `navigable` over a `spectrum` costs nothing but the gesture — drag
  the curve to pan its frequency axis, wheel over it to zoom under the cursor,
  `R` to see all of it again:

  ```python
  spectrum(0, navigable=True, view_start=0.5, view_len=0.5)   # the top half
  ```

  ```ts
  gui.spectrum(0, { navigable: true, viewStart: 0.5, viewLen: 0.5 });   // the top half
  ```

  That window is the element's **own** — normalized over `[0, Nyquist]`,
  `0, 1` being the whole axis — not a navigation group's: nothing else in a
  window measures in hertz along x, so there is no axis to share. It is live
  through {{alt `set(view_start=…, view_len=…)` || `set({ viewStart, viewLen })`}} and comes back as a `"view_x"` event,
  the horizontal sibling of the `"view_y"` every other element's vertical axis
  reports. `spectrum` is also the one view where `navigable` is off unless you
  ask: without it, it is the watching spectroscope.

  The zoom stops where the **analysis** does. A window narrower than a few FFT
  bins across the whole widget shows the interpolation between two neighbouring
  bins — a straight line that no longer answers to the signal — so the host
  floors it at that resolution, computed from the FFT size and the sample rate.
  It is not a fixed fraction of the axis, because a bin is not one: on a log
  axis one is a twentieth of what you see at 500 Hz and a thousandth of it near
  Nyquist. Raise the FFT size to zoom further; that is the only thing that adds
  detail, here as anywhere.

  Because the floor moves with the window, the axis distinguishes the window you
  **asked** for from the one it can **show** you. {{alt `view_start`/`view_len` || `viewStart`/`viewLen`}} are the
  request — set from the {{alt script || page}} or by a gesture, it makes no difference — and the
  axis opens it wherever it is finer than the bins there. So panning down towards
  20 Hz widens the picture on its own, and panning back up hands your window
  back: the trip does not spend the zoom. A window set finer than the
  analysis is honoured the same way — drawn, and reported as `"view_x"`, opened
  up — so what you read back is always what is on the screen.

  And an axis at a bound is quiet: once the window cannot move, the wheel
  reports no `"view_x"` at all. That holds for every view window — `"view"` and
  `"view_y"` too — so a handler counting events is counting movements, not
  notches.

### Where the samples come from

Most of the time you should not have to choose. A **source** is the samples as
something you hold: hand it to the view instead of a carrier, and it picks one
by size — short stays inline, long {{alt spills to a file the host maps || rides a blob beside the JSON}}.

```python
sig = source(take)                          # or source(buffer=b), source(path=p)
v = view(waveform(name="wave", data=sig), title="a take")
win = v.open()

sig.set(other_samples)     # the view redraws; the window is not rebuilt
```

```ts
const sig = gui.source(take);               // or source(null, { buffer: b }), source(null, { path: p })
const v = view({ title: "a take" }, waveform({ name: "wave", data: sig }));
const win = await v.open();

sig.set(otherSamples);     // the view redraws; the window is not rebuilt
```

A source is also the *entry point*, which is what makes it worth holding: one
source in two views is one payload and two references, and `set` reaches the
definition and every window already drawing it. The carrier is fixed when the
source is made — a widget on screen was built around it — so `set` writes
through the same one: inline samples are replaced<!-- python -->, and a spilled file is
rewritten where it is and re-read<!-- /python --><!-- web -->. The threshold is the same in both clients, so a source of the same length makes the same decision; **what differs is the spill**: a page has no temp file, so a source past the ceiling rides a **blob** beside the JSON, its index assigned at `open`. The blob has no live door in the host, so `set` on a spilled source refuses rather than pretending<!-- /web -->. For samples somebody else owns (a server
buffer, a cache) there is nothing to set: change them where they live and call
`sig.reload()`.

The carriers are still there to name by hand, and a view never carries a
megabyte over OSC if it does not have to. In the host's precedence order:

| Prop | What it is |
|---|---|
| `cache` | a prebuilt peak-pyramid file the host {{alt maps || reads}} and renders directly — the most compact path, and the raw samples are never loaded |
| `path` | a file of raw little-endian `f32` the host {{alt maps || reads}} |
| `buffer` | a server buffer number, pulled over the host's client leg |
| `data` | a short list of numbers, inline in the JSON |
| `blob` | a binary blob carried beside the JSON in the same message |

{{api samples_to_file}} and {{api peaks_cache_file}} write the first two from a
sequence of samples{{alt  || , each answering a promise of the path}}; {{api samples_to_blob}} packs the last. A cache follows its samples rather
than being rebuilt: {{api peaks_cache_update_file}} re-summarizes the frame span an
edit touched, and {{api peaks_cache_stream_file}} folds a `/buffer_stream.reply`
report into it — the overview of a take being recorded, measured by the writer
and sent instead of the samples, so a picture reading the file grows as the take
does.<!-- web --> In a page the files are the page's own storage, which is where the host reads a `path` or a `cache` from first; a name the storage does not hold, or a URL, the host fetches from the page's origin, which is how a page draws a file it serves. Under node they are the disk.<!-- /web -->

Those are the primitives, and they are the right call when what you want is
the *file* a `cache` prop names.
To follow a take without a picture in it —
a headless capture, a test, a read-out — `RecordingStream` owns
the whole conversation: it subscribes, keeps one cache per take, and says how
far each has been reported ([Reading the server](data.md#a-take-as-it-records)).

### A redraw does not re-send what it draws

A `/gui_def` describes **every widget in the subtree it names** — that is what
lets a window grow a widget without being rebuilt around it — and a take's
samples are the largest payload in the system. So a panel restated because one
number moved would carry every waveform's audio with it, which is the same
failure as a redefinition taking a reader's zoom, one order of magnitude up.

`KEEP` is the word for it, in place of a value:

```python
from clausters.gui import KEEP, panel, waveform

# The panel is stated again so a label can change; the views that did not
# change name their takes instead of sending them.
panel(waveform(name="a", data=KEEP, channels=1),
      waveform(name="b", data=KEEP, channels=2, label="take"),
      layout="col")
```

```ts
import { KEEP, panel, waveform } from "clausters/gui";

// The panel is stated again so a label can change; the views that did not
// change name their takes instead of sending them.
panel({ layout: "col" },
      waveform({ name: "a", data: KEEP, channels: 1 }),
      waveform({ name: "b", data: KEEP, channels: 2, label: "take" }));
```

The widget is described in full and its bulk is not: the host carries the run it
is already drawing onto the widget that kept its identity. So it is an answer
about a widget the host **has**, which is what naming a widget rather than
leasing its id is for. Said of a widget that is new, or of a name that now draws
something else, it names bulk nobody holds — the host says so in its log and
draws an empty picture rather than inventing one. On a `set` it means *no
change*, so one props table serves both doors.

### The structures are held the same way

Samples are one kind of heavy prop; the others are the **structures** — a
`bpf`'s `points`, a roll's `notes` and `osc`, a patcher's `boxes` and `cords`,
a `score`'s {{alt `display_list` || `displayList`}}. They are the same relation and they take the same
object: name the prop the source *is*, and hand it to the builder in place of
the value.

```python
env = source(points=[(0.0, 0.0), (1.0, 1.0, "exp")])
win = view(bpf(name="env", points=env, editable=True)).open()

env.set([(0.0, 1.0), (0.5, 0.3, "exp"), (1.0, 0.0)])   # every view follows
```

```ts
const env = gui.source(undefined, { points: [[0.0, 0.0], [1.0, 1.0, "exp"]] });
const win = await view({}, gui.bpf({ name: "env", points: env, editable: true })).open();

env.set([[0.0, 1.0], [0.5, 0.3, "exp"], [1.0, 0.0]]);   // every view follows
```

A structure has nothing to choose: it rides in its own prop, which is the
carrier — nothing spills, in either client. What the source adds is that the payload stays **addressable** after
the definition is written — one source in two views is one payload and two
references, `set` reaches the definition and every window already drawing it,
and the value is normalized on the way out exactly as the builder's own option
normalizes it, so both spellings put the same flat list on the wire.

An engraved page is the one that travels two ways, and the source hides it: a
definition carries the display list as its parts (`vb`, `glyphs`, `prims`,
`cursors`, `step`, and what the walk knows about the ids: `elements`, `systems`,
`kinds`) and a live update carries the whole display list, which is
the host's door for replacing a drawing in place. `page.set(...)` picks the
right one.

```python
engraved = source(display_list=score.display_list())
win = view(score_view(engraved, name="page", editable=True)).open()

# a "transpose" event came back: apply it and hand the page back
score.transpose_to(xml_id, position)
engraved.set(score.display_list())
```

```ts
const engraved = gui.source(undefined, { displayList: score.displayList() });
const win = await view({}, scoreView(engraved, { name: "page", editable: true })).open();

// a "transpose" event came back: apply it and hand the page back
score.transposeTo(xmlId, position);
engraved.set(score.displayList());
```

The source takes the display list whole and sends only its drawing layers, so
`{{api page_json}}` — the same layers as a JSON string — is what a **hand-driven**
{{alt `win["page"].set(display_list=…)` || `win.widget("page").set({ displayList: ... })`}} still needs, and not something a source asks
you for.

There is no `reload()` for a structure — it holds its own payload, so there is
nowhere for it to have moved.

### The axes carry the chrome

A ruler, the visible window, the selection, the playhead and the value range
describe the container's **axes**. The shortcuts take them as flat {{alt keywords || options}} and
pack them for you:

```python
waveform(path="take.f32", ruler="beats", tempo=2.0, quant=4.0,
         sel_start=48000.0, sel_len=24000.0, ruler_y="db", link=1)
```

```ts
waveform({ path: "take.f32", ruler: "beats", tempo: 2.0, quant: 4.0,
           selStart: 48000.0, selLen: 24000.0, rulerY: "db", link: 1 });
```

and the general builders take the pair directly, which is what goes on the wire:

```python
signal(view="trace", path="take.f32",
       axes={"x": {"unit": "beats", "tempo": 2.0, "quant": 4.0, "link": 1},
             "y": {"unit": "db", "min": -1.0, "max": 1.0}})
```

```ts
gui.signal({ view: "trace", path: "take.f32",
             axes: { x: { unit: "beats", tempo: 2.0, quant: 4.0, link: 1 },
                     y: { unit: "db", min: -1.0, max: 1.0 } } });
```

On the **x** axis: `unit` (`"time"`/`"samples"`/`"beats"`/`"off"`),
`start`/`len` (the navigation window), `tempo`/`beat_at`/`quant`,
`{{wire sample_rate}}`, `link`, `sel_start`/`sel_len` and the `playhead` family. On
**y**: `unit` (`"norm"`/`"db"`/`"bits"`/`"percent"`/`"hz"`/`"off"`),
`start`/`len`, `min`/`max`, `bit_depth`, `sel_min`/`sel_max`. These are the wire's keys, the same in both clients.

**`link` is the interesting one.** Views naming the same `link` id form a
**navigation group**: one horizontal window, one selection, one playhead,
shared. Zoom, pan or drag a selection on any member and all of them move —
including the ones nobody is looking at, since membership is read from the tree
and not from what is on screen. Only the vertical window stays per view, on
purpose: a waveform's y is amplitude and a spectrogram's is frequency, and no
single number could mean both. The selection's **value band** stays per view for
the same reason — a sweep with height on a waveform restricts that view to a
range of amplitudes and leaves the spectrogram beside it showing the whole
band, while the time span they share moves for both.

```python
view(
    waveform(name="wave", data=take, ruler="time", link=1),
    spectrogram(data=take, link=1),
    layout="col")
```

```ts
view({ layout: "col" },
     waveform({ name: "wave", data: take, ruler: "time", link: 1 }),
     spectrogram({ data: take, link: 1 }));
```

Scroll one; the other follows.

## The multitrack, and the ruler over it

A **multitrack is one widget**, and it is the multitrack editor's.
`edit(multitrack)` over a `Multitrack` opens it: the shared
crate draws the document — a row per track, a box per region, the automation
over both — and every edit a hand makes there lands on the document, in its
history, rather than in the widget (the [document](document.md) chapter
is where that is taught). There is no builder for it on this surface: a
multitrack drawn with nothing behind it would have nowhere to keep what a hand
did to it.

A `field` is what is left of the old three-in-one container: the free-standing
**ruler** of a navigation group (`timeruler`), a strip with nothing placed on
it. It is its **own** strip above whatever it rules rather than part of it, so
ruling a stack costs none of its members a pixel; a ruler with no `link` joins
the window's own group.

## The rest of the elements

```python
from clausters.gui import (bpf, canvas, code, curve, keys, meter, nodes,
                           nodetree, piano, post, score)
```

```ts
import { bpf, canvas, code, curve, keys, meter, nodes, nodetree, piano, post, score } from "clausters/gui";
```

| Element | Shortcut | What it is |
|---|---|---|
| `curve` | `bpf` | a drawable break-point envelope, played by the server's own shape math. `points` of `(t, v[, shape])` or an `Env` through `{{api env_to_points}}`; edits come back as `"points"` |
| `keys` | `piano` | a playable keyboard with real piano proportions. `min`/`max` are the visible range (its overview strip pans and zooms it), `{{api active_min}}`/`{{api active_max}}` gray the keys outside a mapping, and a `voice` naming a def has the **host** manage one server voice per held key |
| `nodes` | `nodetree` | the audio server's node graph, live, with each synth's controls |
| `meter` | — | a bus level, read from the server's shared segment every frame with no messages at all |
| `score` | — | an engraved notation page: the client engraves and sends a display list, the host fits and tessellates it. A click names an element, a drag on a note transposes it (only what has a pitch drags) |
| `canvas` | — | a {{alt script-supplied || page-supplied}} WGSL shader over the widget area, fed by `params` and by control `buses` |
| `label` | — | static text: `{{api text_size}}`, `wrap`, `align` |
| `code` | — | an editor of code in the monospaced face: line numbers, the line the caret is on, selection by every hand, the host's clipboard, undo by typing run, and indentation by a level (`indent`) with Enter keeping it and opening one after a block in `language`, which also colors the text by its syntax in the theme's `syntax_` roles; brackets and quotes typed close, Ctrl+/ comments the lines selected, the bracket by the caret is marked with its pair, and a bar finds and replaces (Ctrl+F) or goes to a line (Ctrl+Alt+G). Tab indents in it, Ctrl+Tab leaves it; its text is read by a query, and `"dirty"` says when it stops or starts being the one marked `saved` |
| `post` | — | a post window: the lines a session writes, appended by a set of their kind ({{alt `set(out=text)` || `set({ out: text })`}}, `err`, `result`, `info`) and emptied by `clear`. It keeps `keep` lines, follows the newest while the reader is at the end, and copies what a hand selects; only the rows in view are measured and drawn |

`curve`, `nodes` and `keys` are the model's names; `bpf`, `nodetree` and
`piano` are the same builder under the name the catalog used. The `notes`
element, the piano roll, has no builder: it is the notes editor's, opened over
an `EventSequence` with `edit` ([the document](document.md)).

### An engraved page

The `score` widget draws a page of music, and what it draws is a **display
list** — a glyph-outline table plus placed glyphs, staff lines, stems and beams
in page units — which the client produces and the host tessellates. The host
reads no notation and links no engraver; that split is what lets one host
renderer serve every client. The engraving is {{alt `clausters.notation` || `clausters/notation`}}, the same layer in both clients:
the score model, the SVG walk and the MEI encoder are the shared Rust core, and
the engraver is [verovio](https://verovio.org) from the same pinned sources, with the same options,<!-- web --> compiled to wasm<!-- /web --> — a page and a
window engrave one score into one drawing, which a parity suite compares
primitive by primitive. [Timelines](timelines.md#seeing-a-timeline-as-a-score) is where a score is read, edited and heard; what is the widget's is here.

```python
from clausters import Event, notation
from clausters.gui import score_view, view

melody = [Event(midinote=m, dur=1.0) for m in (67, 69, 71, 72)]
score = notation.Score.from_notes(melody, meter="4/4", key="G")

page = score.display_list()          # what is drawn, the cursors, the notes
win = view(score_view(page, name="page", editable=True), title="score").open()
```

```ts
import { Event, notation } from "clausters";
import { scoreView, view } from "clausters/gui";

const melody = [67, 69, 71, 72].map((midinote) => new Event({ midinote, dur: 1.0 }));
const score = await notation.Score.fromNotes(melody, { meter: "4/4", key: "G" });

const page = score.displayList();    // what is drawn, the cursors, the notes
const win = await view({ title: "score" },
    scoreView(page, { name: "page", editable: true })).open();
```

<!-- web -->
Opening a score is the layer's one asynchronous step, and only because the engraver is **fetched on demand**: nothing in the runtime imports it, so a page that draws no notation never downloads it. Everything after that — drawing, editing, undo — is direct.
<!-- /web -->

An engraved page comes in three layers from one engraving, because the engraver
mints fresh MEI ids on every load and ids from two engravings do not line up:
what the host **draws**, where the **cursor** goes at each onset, and what
**sounds** (one `{t, dur, pitch, id}` per note, which stays in your {{alt script || page}} — it
is what a driver plays).

Editing rides on those ids. A click reports the element under the cursor; a
vertical drag on a note reports the diatonic staff position the note *reaches* —
absolute, so an edit that arrives twice moves nothing the second time. Only a
note drags: the page says what each id is (`kinds`), and a slur, a time
signature or a rest is selected and never displaced. Your {{alt script || page}} applies
it and sends the new page back — through a source, as above, or by hand:

```python
def on_page(tag, element, position):
    if tag == "transpose":
        score.transpose_to(element, round(position))
        win["page"].set(display_list=page_json(score.display_list()))

win["page"].on_event(on_page)
```

```ts
win.widget("page").onEvent((tag, id, position) => {
    if (tag !== "transpose") return;
    score.transposeTo(id, Math.round(position));
    win.widget("page").set({ displayList: pageJson(score.displayList()) });
});
```

`score.undo()` and `score.redo()` are the client's, not the host's — the host
holds no score at all — and what they walk is the **editing context's** pile, as [Timelines](timelines.md#opening-a-score-that-was-only-a-document) says.

To **play** the page with the cursor following the sound, `PlayheadSync` is
the same object every time view uses — a multitrack, a piano roll, an engraved page —
and the notation layer's playhead sync only fills in the page's unit, since a score places
its cursor in milliseconds where a multitrack places it in samples. It holds no tempo:
beats cross through the map of the timeline a pass plays. The line is the **host's**: `play` sends one anchor — the clock value the view's
time 0 maps to — and the host sweeps from it every frame, so a pass costs one
message and not one per frame. {{alt `examples/notation/score_from_data.py` || `examples/notation/score-from-data.html`}} is the example.

## Planes: a workspace and a patcher

A `plane` is the container with **two axes locked to one scale** — a pannable,
zoomable plane in content units. Drag the empty background to pan, wheel to
zoom anchored at the cursor:

```python
from clausters.gui import plane, scroll

scroll(panel(x=0.0, y=0.0, w=200.0, h=1200.0),
       axis="y", zoom=False)          # a plain vertical scroll view
plane(a, b, axis="both")              # the full free plane
```

```ts
import { plane, scroll } from "clausters/gui";

scroll({ axis: "y", zoom: false },
       panel({ x: 0.0, y: 0.0, w: 200.0, h: 1200.0 }));   // a plain vertical scroll view
plane({ axis: "both" }, a, b);                            // the full free plane
```

The constrained scroll views are configurations of it, not different
containers. With `boxes` and `cords` the same plane is the **patcher** —
boxes with inlets on top and outlets on the bottom, a cord per connection,
coloured by rate:

```python
from clausters.gui import patch

patch(boxes=[{"def": "source", "inlets": [], "outlets": ["out"]},
             {"def": "sink", "inlets": ["in", {"name": "gain", "rate": "control"}],
              "outlets": []}],
      cords=[0, 0, 1, 0])
```

```ts
import { patch } from "clausters/gui";

patch({ boxes: [{ def: "source", inlets: [], outlets: ["out"] },
                    { def: "sink", inlets: ["in", { name: "gain", rate: "control" }],
                      outlets: [] }],
            cords: [0, 0, 1, 0] });
```

Drawing a cord emits `"wire"`, dragging a box emits `"move"` — the driver owns
the graph and the geometry, and re-renders.

## Style, and what a drag means

Every chrome colour is a named role in one theme. Any container may carry a
`theme` — a partial table overlaying its parent's for the whole subtree, a
**theme group** — and any widget a single `color` that re-seeds the roles
carrying its function:

```python
panel(knob(color="#40c0a0"), theme={"panel_fill": "#101018", "accent": "#c08040"})
```

```ts
panel({ theme: { panel_fill: "#101018", accent: "#c08040" } }, knob({ color: "#40c0a0" }));
```

Both are live through `set` (a theme rides as its JSON string), and a theme on
the root persists with a named def, so a bundle ships its look.

Two props shape the drawing rather than its colour, both on any widget and both
live: `opacity` (`0`–`1`) and `radius` (a corner radius in logical pixels).

```python
panel(button(label="arm", radius=6), opacity=0.5)
```

```ts
panel({ opacity: 0.5 }, button({ label: "arm", radius: 6 }));
```

`opacity` behaves like a theme group — it multiplies down the whole subtree, so
a control at `0.5` inside a panel at `0.5` draws at `0.25` — while `radius`
applies to the widget alone; a negative number clears either. Each box clamps
the radius to half its shorter side, so a widget's own frame rounds and the
hairlines inside it keep their shape. What the fade covers is the flat drawing:
the chrome, the controls and the text. A heavy view's picture — a waveform's
trace, a spectrogram's texture, a `canvas` shader — is drawn by its own
pipeline and keeps its own opacity, and two overlapping shapes inside a faded
widget show through each other, because the fade is per-shape and not a layer.

Antialiasing is not a prop at all: smoothing every edge is one setting of the
**host** — four samples by default<!-- python -->; `--msaa 1`, or `msaa = 1` under `[gui]`,
turns it off<!-- /python --> — because it is the render pass that is multisampled: one
attachment per window, nothing per widget.

The **typeface** is the host's too, for the same reason. It carries the one it
draws with — a subset of DejaVu Sans, with a subset of Noto Sans Devanagari
it falls back to, the same in a native window and in a page, and shaped:
Arabic joins, a mark stands over its base, and a right-to-left word reads from
the right — and another may be handed over at any point. Reaching its bytes is the platform's half ({{alt a script reads a file || a page fetches a URL}}), and *handing them over* is the protocol's — `/gui_font`, the same call in both clients:

```python
gui.font(Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf").read_bytes())
```

```ts
const face = await fetch("/fonts/DejaVuSansMono.ttf").then((r) => r.arrayBuffer());
host.font(new Uint8Array(face));
```

A raw TrueType/OpenType file<!-- web --> (the rasterizer does not decompress WOFF2, so a Google Fonts CSS URL is not one), served with CORS if it comes from another origin — a CSS `@font-face` cannot serve here, since the host draws into a canvas and never reads the document's fonts<!-- /web -->, drawn by every window the host has open and every
one it opens later — a face is a property of the host, not of a window, so the
call carries no id. Loading one **relayouts nothing**: the sizing table never
followed the typeface, so the same tree comes up the same size before and after.
What changes is that `{{api text_size}}` becomes continuous rather than quantized to
half-steps of the cell, which a bitmap glyph's own pixels require. The
launch-time spelling is the host's `--font`<!-- python --> (`GuiProcess(font=...)`), for a face
that should be in place before the first window opens. A host built without
the rasterizer draws with its embedded bitmap face, the floor under every
build; bytes the host cannot read leave it on the face it had<!-- /python -->.

Panning, sweeping a selection and locating the transport are the
**container's** gestures, so any container may carry a `gestures` table keyed
by modifier chord, each value a plan of steps in order:

```python
waveform(data=take, gestures={"drag": "pan", "shift": "select"})
```

```ts
waveform({ data: take, gestures: { drag: "pan", shift: "select" } });
```

The steps are `element` (hand the press to whatever is under the cursor, which
may decline), `pan`, `select`, `select_box`, `marquee`, `locate` and `none`. The
order is the point: `"element marquee"` is a multitrack's — grab the clip under the
cursor, and if there is none, sweep for the clips. A plan that consumes nothing
falls outward to the container around it.

`marquee` and `select` are **two selections, not one gesture with two
pictures**. A marquee selects the *objects* a rectangle covered — a multitrack's
clips, a patcher's boxes — and its rectangle is the gesture's own picture, gone
when the hand lets go; what stays is what is selected. `select` sweeps a *time
range*, which is a state the group keeps and draws as a band. A time range over
a multitrack is a real selection and a valid thing to want: it is asked for by
name, a plain drag's step or one on a modifier.

`select` sweeps the **time span**; `select_box` sweeps the same span
**restricted to the band of values** it covered, which is a rectangle rather
than a stripe. The second one declines where the picture has only one measured
axis, so `"select_box select"` is the plan for a stack of heavy views: a
rectangle on a waveform, whose y is amplitude, and the plain span on a
spectrogram, whose y is frequency — a range of *bins*, which is a different
field of a selection and a gesture that does not exist yet. A plain drag stays
a time span everywhere on purpose: that is what a drag over a waveform means in
every editor, and what a band of values is *for* is your business, so you name
the step.

### The clipboard: copy is the host's, cut and paste are yours

Ctrl+C over a selection is a **read**, so the host makes it alone: the span
leaves the samples it has mapped and lands on its own clipboard, typed, with
the rate it was taken at — nothing reaches your {{alt script || page}}. Where it cannot read the
source (a peak overview has no samples behind it; a live view has no addressable
past) it says so with a `"refused"` event rather than copying silence.

Ctrl+X and Ctrl+V **change data**, which the host does not own, so they arrive
as requests. A paste brings the clipboard with it — the kind, the whole typed
document, and one blob per bulk payload — because the clipboard is the host's:
a block copied in one window is pasted against an owner that never saw the copy.

```python
def on_clip(tag, *vals):
    if tag == "paste" and vals[1] == "samples":
        doc, blob = json.loads(vals[2]), vals[3]
        block = doc["content"]          # channels, frames, sample_rate
        values = array.array("f"); values.frombytes(bytes(blob))
```

```ts
function onClip(tag: string, ...vals: any[]) {
    if (tag === "paste" && vals[1] === "samples") {
        const doc = JSON.parse(vals[2]), blob = vals[3] as Uint8Array;
        const block = doc.content;         // channels, frames, sample_rate
        const values = new Float32Array(blob.buffer, blob.byteOffset, blob.byteLength / 4);
    }
}
```

The rate travels with the block and nothing converts it: resampling is an edit,
and an edit is something you perform and log, never a side effect of a paste.

Over a multitrack the two verbs mean the same and are answered by whoever holds
the document: a cut whose selection covers a clip removes that placement
(undoably), and a cut *across* a clip — or a paste of samples — is refused with
its reason, because that is a new length for the samples under it and samples
belong to whoever owns them.

The audio editor's clipboard holds **a copy of segments**: the spans of files the selection is made of, by path, so a copy moves no sample and costs nothing however long it is. Cut removes the span only after the copy is published. Paste and Paste mixed work across files and windows, even after closing the origin: the files the clipboard names are kept while it names them, renders included. Text is refused. A block of another width goes by the paste's rule: a mono block is duplicated on every channel, a block pasted into a mono file is mixed (the average of its channels, rendered into a file of its own), and otherwise its channels land one by one from the first, those past the file's dropped and the file's others silent over what was put in. Paste preserves a block's sample rate through the parts ratio; Paste mixed requires matching rates and renders the mix into a file of the destination's own. There is no implicit resampling. Clipboard replacement cannot alter a previous paste or its history.<!-- python --> New system text is treated as text, even if it looks like a typed clipboard document.<!-- /python --><!-- web --> The host clipboard stays page-wide and does not acquire system clipboard access.<!-- /web -->

If an audio edit and its repair both fail, resolve the I/O problem and call `{{api editor.retry_pending}}()`. It retries pending repairs without repeating the failed edits. Successful repair allows editing, saving and closing again; another failure keeps the affected files protected and reports an error.

Audio edits and saves are prepared before their server steps run and confirmed only when those steps succeed. A failed save preserves the previous destination and leaves unsaved edits dirty. Saving captures a content revision: editing during the write stays dirty, while undoing to the saved revision is clean. A late result addresses its original file and cannot rebind a different file's views.<!-- python --> A save over the file the take reads holds that file until it lands, so an edit waits for it; a save elsewhere does not, since the take reads only files that never change.<!-- /python --> Cleanup waits for its server acknowledgement before returning a buffer id for reuse; a failed cleanup remains reserved for retry or connection invalidation.

## The instrument without the script: a bundle

Everything so far kept the {{alt script || page's code}} in the loop. A **bundle** is the other posture:
the instrument written to a directory, and the {{alt script || code}} gone. The GUI is what
drives it, because its widgets are bound.

The parts are the ones you have already seen, plus two kinds of hole. A
**symbol** is something the instrument allocates — a bus, a node, a buffer —
and a **parameter** is a value the mount supplies:

```python
from clausters.bundle import Bundle
from clausters.gui import knob, label, meter, panel, scope, toggle, view

b = Bundle("fm-voice")
freq = b.param("freq", float, default=220.0, min=60.0, max=700.0)
node = b.node("graph")        # -> "@graph"
lfo  = b.bus("lfo")           # -> "@lfo"

voice = b.synthdef(fm_voice())          # named "fm-voice.voice"
trem  = b.synthdef(tremolo())
graph = b.graphdef(rig(voice, trem))    # a GraphDef: members wired by buses

b.gui(view(
    panel(label("the GraphDef's surface", weight=3),
          toggle(label="play", value=True, bind=["/node_run", node], weight=1),
          layout="row", h=30),
    panel(knob(label="freq", min=60.0, max=700.0, value=freq,
               bind=["/node_set", node, "freq"]),
          knob(label="depth", min=0.0, max=1.0, value=0.5,
               bind=["/node_set", node, "depth"]),
          layout="row"),
    panel(meter(lfo, rate="control", min=0.0, max=1.0, label="lfo"),
          scope(lfo, rate="control", min=0.0, max=1.0),
          layout="row"),
    title="FM + tremolo", w=680, h=400, layout="col"))

b.boot(["/graph_new", graph, node, 0, 0, "lfo_bus", lfo, "freq", freq])
b.preset("bright", freq=110.0)
b.write("./fm-voice")
```

```ts
import { loadCore } from "clausters/page";
import { Bundle } from "clausters/bundle";
import { knob, label, meter, panel, scope, toggle, view } from "clausters/gui";

await loadCore();
const b = new Bundle("fm-voice");
const freq = b.param("freq", "float", { default: 220.0, min: 60.0, max: 700.0 });
const node = b.node("graph");           // -> "@graph"
const lfo = b.bus("lfo");               // -> "@lfo"

const voice = b.synthdef(fmVoice());    // named "fm-voice.voice"
const trem = b.synthdef(tremolo());
const graph = b.graphdef(rig(voice, trem));   // a GraphDef: members wired by buses

b.gui(view({ title: "FM + tremolo", w: 680, h: 400, layout: "col" },
    panel({ layout: "row", h: 30 },
          label("the GraphDef's surface", { weight: 3 }),
          toggle({ label: "play", value: true, bind: ["/node_run", node], weight: 1 })),
    panel({ layout: "row" },
          knob({ label: "freq", min: 60.0, max: 700.0, value: freq,
                 bind: ["/node_set", node, "freq"] }),
          knob({ label: "depth", min: 0.0, max: 1.0, value: 0.5,
                 bind: ["/node_set", node, "depth"] })),
    panel({ layout: "row" },
          meter(lfo, { rate: "control", min: 0.0, max: 1.0, label: "lfo" }),
          scope(lfo, { rate: "control", min: 0.0, max: 1.0 }))));

b.boot(["/graph_new", graph, node, 0, 0, "lfo_bus", lfo, "freq", freq]);
b.preset("bright", { freq: 110.0 });
await b.write("./fm-voice");
```

Each knob carries an inline `bind` — the same binding as the widget handle's `bind`,
declared in the tree so that a def which is only ever *loaded* still arrives
wired. `/graph_new` in the boot list brings the instance up: its own node id,
its own LFO bus, and its parameter as an initial port value.

Open it with no {{alt script || client}} at all:

```sh
# from crates/clausters-gui
cargo run --features standalone --bin clausters-gui -- \
    --standalone fm-voice --data-dir ./fm-voice
```

The window comes up, the graph starts, the knobs drive it and the meter reads
the bus — with nothing running but the host and the server embedded in it.

Two authoring rules follow from the holes living only in the GuiDef and the
boot list. **A bus, a node or a buffer reaches a def as a control**, never as a
baked constant, or two instances would write the same bus. And **widget ids are
local** to the bundle — the root is 1 and the rest are yours from 2 up, since
the mount offsets the whole block per instance. `write` checks both before it
emits anything: an unmountable bundle is unwritable.

The same directory is what a browser tab mounts as a custom element, one line
of markup and no client library at run time. That leg — the format, the mount's
two phases, several instruments in one document — is the
[bundles chapter](bundles.md) and the web client's
{{alt [components chapter](https://clausters-web.readthedocs.io/en/latest/components.html) || [components chapter](components.md)}}.

## Where to go next

- The GUI host wired to a running audio server is one call:
  `session.gui()` returns the same `GuiHost`, wired to the
  session's server<!-- python --> and mapping the same shared segment<!-- /python --> — which is what lets a
  meter, a scope and a playhead read the engine with no per-frame messages.
- Every builder's own props, defaults and event payloads are in the
  {{alt [API reference](api/clausters.gui.guidef.md) || [API reference](api/Namespace.gui.md)}}.
- The wire underneath — the `/gui_*` commands, the event payloads, the axis
  properties — is the server guide's
  [GUI protocol chapter](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
- A multitrack is drawn by one `multitrack` widget, and what it holds is the
  document's: the [document chapter](document.md) is the layer above.
- The runnable demos live in {{alt `clients/python/examples/` || `clients/web/examples/`}}, one capability each.
- Everything above used the shortcuts, which is what a {{alt script || page}} reaches for.
  [Building from the model alone](gui/model.md) writes the same kind of window
  without any of them — the four containers, the elements and `node` itself —
  which is both how to say what no shortcut names and what the wire actually
  carries.
