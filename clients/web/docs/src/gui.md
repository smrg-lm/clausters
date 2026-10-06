# The visual elements in a page

The GUI host is the same host the desktop runs, compiled to wasm and drawing on
a `<canvas>` in your document — so the widget tree, the props, the events and
the bindings are the ones the [Python client's GUI
chapter](https://clausters-python.readthedocs.io/en/latest/gui.html) walks
through, and that page is the tutorial. This one says what the browser makes
different, and nothing that book already says.

**The host is what draws, always** — and in a page that is worth saying twice,
because a page has a `<canvas>` of its own and the temptation is right there.
A script names what to look at (`plot`, `scope`, a `waveform`/`scope`/
`spectrum` widget) and the host reads the bus or the buffer and paints it; this
client computes no picture — no pixel column, no trigger, no decibel curve —
and neither does the Python one. The two are one client in two languages, and a
figure drawn two ways is a difference nothing checks. Drawing your own canvas
over the [data paths](data.md) is of course allowed, and it is *your* program:
not a surface this package provides, documents or keeps in step.

## The model, in one table

A GuiDef node is `{id, type, props, children}`, and a `type` names one of three
things:

| Kind | What it is | Types |
|---|---|---|
| **Container** | owns 0, 1 or 2 **axes**, and so a coordinate system its children are placed in | `window`, `layout`, `plane`, `field` |
| **Element** | draws against the axes of the container holding it | `signal`, `notes`, `curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label`, `separator`, `progress` |
| **Control** | an element with a value and no axis | `slider`, `knob`, `number`, `button`, `toggle`, `text`, `choice` |

The builders named after the old catalog — `panel`, `stack`, `scroll`,
`waveform`, `plot`, `scope`, `spectrum`, `spectrogram`, `phasescope`,
`timeruler`, `pianoroll`, `bpf`, `piano`, `nodetree`, `patch` —
are **shortcuts** onto those nodes with the props of one common case. `layout`,
`plane` and `signal` are the general ones beside them; a `field` is only a
free-standing ruler, which `timeruler` writes. Both emit the
same JSON the Python builders emit, vector for vector; the wire is the shared
thing, the language surface is not.

## What the browser changes

**camelCase options, one bag.** Where Python takes keyword arguments in
snake_case, the TypeScript builders take one options object in camelCase, and
the children come after it:

```ts
import { gui } from "clausters";

gui.view(
    { title: "filter", w: 520, h: 260, flow: "col" },
    gui.label("a filter", { h: 24 }),
    gui.panel(
        { flow: "row" },
        gui.knob({ name: "cutoff", label: "cutoff", min: 20.0, max: 20000.0, value: 800.0 }),
        gui.knob({ name: "res", label: "res", min: 0.0, max: 1.0, value: 0.3 }),
    ),
    gui.signal({
        name: "wave",
        view: "trace",
        path: "take.f32",
        navigable: true,
        axes: { x: { unit: "beats", tempo: 2.0, link: 1 }, y: { unit: "db" } },
    }),
);
```

`textSize` becomes `text_size` on the wire, `baseBucket` becomes `base_bucket`,
`viewZoom` becomes `view_zoom`. Inside an `axes` pair the keys are the wire's
own (`sample_rate`, `sel_start`), since that object goes through untouched.

**A builder returns a `View`, and the view opens itself.** The tree is the
subject of the sentence — `view(...).open()` rather than `host.open(view(...))`
— the way a def is (`synthdef.send(server)`), and there is one root builder,
not two: a view with a parent is a component, a view with **no** parent is the
window, and any node opens (`knob({...}).open()` is a window that is a knob).
The document is unchanged: a `View`'s own properties are the JSON, so
`JSON.stringify` writes what it always wrote. What is added is the name index
(`v.find("cutoff")`, `v.names()`) and `open`; the bracket stays the document
key, because the two addressings cannot share one. `window` is the older
spelling of `view` and still works.

**No widget carries an id.** You pass `name` instead, and `open` hands back a
window handle you index by name. The ids are allocated **in the document that
goes out**, never written into the tree you wrote — which is what lets one view
open as many times as you like, each window with ids of its own, and what makes
the same subtree nested twice two widgets rather than one.

**The element goes in `open`, and a view that names none gets a canvas of its
own.** `view(...).open(el)` mounts the view into that element's box — the canvas
inside it is made and fitted for you, so `attach`/`fit` stop being something a
script writes — and `view(...).open()` appends a canvas to the document. That is
how *a view with no parent is a window* finishes in a page: **a view with no
element is a canvas**, so a document holds as many as it opens, and
`win.canvas` is the one this window draws on. A host reached over a socket has
windows of its own and refuses an element.

**The samples are a `source`, not a carrier.** `waveform({ data: sig })`, where
`sig = source(samples)` decides how they travel and stays addressable —
`sig.set(other)` rewrites the definitions holding it and pushes to every live
widget. The threshold is the Python client's, so a source of the same length
makes the same decision in both; **what differs is the spill**. A page has no
temp file, so a source past the ceiling rides a **blob** beside the JSON and its
index is assigned at `open` — which is `blob: 0` stopping being a
correspondence kept by hand. The blob has no live door in the host, so `set` on
a spilled source refuses rather than pretending; a native client rewrites its
file and re-reads it.

**A structure is a `source` too, and there the two clients are identical.** A
`bpf`'s `points`, a roll's `notes` and `osc`, a patcher's `boxes` and `cords`, a
`score`'s `displayList`: `source(undefined, { points })` names the prop it is,
`bpf({ points: env })` takes it in place of the value, and `env.set(...)`
rewrites the definitions and every live widget. Nothing spills — a structure
rides in its own prop, which is its only carrier — so the paragraph above has no
counterpart here: the option is spelled `displayList` where Python spells
`display_list`, and that is the whole of the difference.

**A control widget is built from the control it drives.** `knob(freq)`,
`slider(sd.control("amp"))`, `number`, `toggle` and `button` read the control's
**name** and **default** off it, so the widget and the graph cannot disagree about what
`"freq"` is; the **range is the widget's** (`knob(freq, { min: 110.0, max:
880.0 })`), since a control is a signal in a graph and says nothing about how a
knob is drawn. Only a Faust parameter arrives with a range, from the `hslider`
that declared it. All three def families answer with the same `ControlInfo`
shape, spelled `sd.control("freq")` where Python writes `sd["freq"]`: a class
here cannot take a bracket without an index signature over everything else on
it. Then the whole surface binds in one verb — `win.bind(synth)`,
`win.unbind()`, `win.controlMap()` — and `win.widget("freq").bind(...)` is still
there for a bus, another widget or an arbitrary address.

The two switches spell the same thing in both clients and need no idiom of their
own: `button(gate, { label: "hold" })` sends `on` while it is held and `off` when
it is let go, `button(fire, { mode: "press" })` sends one message and nothing
after it, and both switches carry the pair `on`/`off` (`1`/`0` by default) rather
than a boolean. Building a `"press"` button over a control that is not a trigger
throws here too, for the same reason: the press would leave `on` standing
forever.

**What the hand did** has three verbs of its own, camelCase being the whole
difference: `onPress`, `onRelease` and `onClick`, where Python writes
`on_press`/`on_release`/`on_click`. `onClick` is the completed press — the
pointer came up while still on the button, so sliding off first cancels it —
and all three reach the script whether or not the widget is bound, because a
binding forwards a *value* and a command is not one. `onEvent` stays the raw
stream and sees them too.

**Nothing is pumped.** The page already has an event loop, so the Python
client's `pump` has no counterpart: a handler fires when the message arrives.
Building is synchronous; opening is awaited (the host may still be booting, and
resolving the ambient one is what awaits), as is anything that waits for the
host to *answer*:

```ts
const win = await tree.open();                     // a WindowHandle
win.widget("cutoff").set({ value: 2000.0 });
win.widget("cutoff").onEvent((v) => console.log("cutoff ->", v));
const info = await win.widget("cutoff").query();   // a round trip, so awaited
```

Getting the host is the asynchronous part, since it loads wasm:
`const host = await new GuiHost().boot();`.

**Waiting for a window to close** is the one place the reference client's shape
does not cross. There a *script* must not exit while a window is on screen, so
`wait()` blocks its thread; a page never exits and must never block, so the same
verb answers a promise — and it is not how a program stays alive, it is how one
sequences on the close:

```ts
win.closed;                  // false until it goes
await win.wait();            // true when it closed
await win.wait(10);          // false if it is still there after ten seconds
await host.wait();           // when every window this host opened is gone
```

`closed` and `wait` are on everything that opens a window — a `WindowHandle`,
what `plot`, `scope` and `plotDef` answer, and an `Editor`.

**Two ways to have a host, and one ambient rule.** `boot()` brings up a wasm host this handle
owns; `attach()` connects to one it did not open — the host already up in this
page, or, with `transport: "ws"`, a *native* `clausters-gui --ws` driven from
the tab, which is the same object over a different carrier. Either becomes the **ambient** host if none is registered (first-wins,
the mirror of the audio server's default-session adoption), which is why
`view(...).open()` needs no argument, and `stop()` gives the registration up. A
`Session` opened with `Session.embed()` carries one either way; a session drives
a host it did not open by being **given** one — `new Session(server, clock,
await new GuiHost({ transport: "ws", url }).attach())` — the way it is given a `Server`.
`newGuiHost()` boots an instance that is **not** the page's — its own engine
unless you hand it one — for a document holding several independent
instruments. It appends no canvas of its own; its views take the elements they
are opened in, like any other.

**Bulk data is fetched, not mapped.** A `path` or a `cache` is a URL the host
fetches rather than a file it maps; a `buffer` is still pulled over the host's
client leg. Everything else about the source precedence is identical.

**The canvas is an element, and the document places it.** There is no window
manager: a GuiDef rooted in a `window` draws into a canvas that CSS sizes — one
per def, so several windows are several canvases. `open` makes each view its
own (in the element you name, else appended to the document) and hands it back
as `win.canvas`; closing the window releases it. Sizes stay what they always
were — logical pixels resolved through the page's `devicePixelRatio` — so a tree
written for the desktop comes up the same size in a tab. That substitution is
what makes a bundle mountable in the flow of a page: see
[Components](components.md).

A def fed straight through the binding surface, with nobody having said where,
draws on the page's **fallback** canvas — appended the first time that happens,
so a document whose views all name their own place never carries an empty one.

**The window's chrome is the host's, here as on the desktop.** A window's menu
bar, a button's menu, a context menu, a tip, the list a `choice` opens and a
dialog all open in the host's popup layer, over the canvas and inside it --
never as DOM. The page answers the secondary button for a canvas whose tree has
a `context` menu under the pointer, and a finger held still is the same
request:

```ts
const bar = gui.menu(
    gui.entry("File", undefined, { menu: gui.menu(gui.entry("Open", "open"), "-",
                                                 gui.entry("Loop", "loop", { checked: false })) }),
    gui.entry("About", "about"));
const win = await gui.view({ menu: bar }, tools, work).open();
win.handle().onEvent((tag, ...payload) => console.log(tag, payload));
```

`gui.toolbar`, `gui.tabs`, `gui.pager`, `gui.dialog` and `gui.fileDialog` build
the same compositions as in Python, `gui.table` the same rows of data, and `gui.ICON` names the glyphs the host's own face
draws. A character the host has no face for -- a symbol of a music font -- is
still an icon when the window brings its outline: `gui.view({ glyphs: { E1D5:
path } })` maps a codepoint to the SVG path of its shape, and from then on that
character is drawn as the shape wherever text is. A `files` chooser lists the page's own storage (OPFS) -- a path in a tab
is a path there, and `/` its root -- where the same chooser lists the disk in a
window. `examples/panels/chrome.html` holds one of each.

**The keyboard is shared with the page.** A canvas is focusable, and while it
holds the focus the host reads the keys: click a `text` field to type into it,
Tab to walk the window's focusable widgets -- every control is one, and the
arrows, Space and Enter work it. **Tab past the last one gives the
keyboard back to the document** — the canvas blurs and the browser's own tab
order carries on — so a GuiDef mounted in the flow of a page is never a
keyboard trap. A script points the focus itself with
`win.widget("name").focus()`, and hears every move as a `"focus"` event.

**The host draws text with a typeface it carries, and a page may hand over
another.** The face is compiled into the host — a subset of DejaVu Sans, the
same in a page and in a native window — so text needs nothing fetched. To draw
with another, reaching its bytes is the platform's half (a page fetches a URL,
a native host maps a file), and *handing them over* is the protocol's:
`host.font(bytes)` is `/gui_font`, the same call in both clients, and the
launch-time spelling is the host's own `--font <path>`.

```ts
const face = await fetch("/fonts/DejaVuSansMono.ttf").then((r) => r.arrayBuffer());
host.font(new Uint8Array(face));
```

It must be a raw **TrueType/OpenType** file (the rasterizer does not decompress
WOFF2, so a Google Fonts CSS URL is not one), served with CORS if it comes from
another origin — a CSS `@font-face` cannot serve here, since the host draws into
a canvas and never reads the document's fonts. A face is a property of the
**host**, not of a window, so the call carries no id. Loading one relayouts
nothing: the sizing table never followed the typeface, so the same tree comes up
the same size before and after, and it may be handed over at any point. What
changes is that `textSize` is then continuous rather than quantized to
half-steps of the cell, which a bitmap glyph's own pixels require.

Composition (IME) and the system clipboard stay the **page's**: a canvas cannot
host an input method, so the host reads the keys it is handed and no more, and
the clipboard a field cuts and pastes through is its own, page-wide. Text that
needs composing is not entered through a host field today.

## Notation: engraving in the page

The `score` widget draws a page of music, and what it draws is a **display
list** — a glyph-outline table plus placed glyphs, staff lines, stems and beams
in page units — which the client produces and the host tessellates. The host
reads no notation and links no engraver; that split is what lets one host
renderer serve every client.

The engraving is `gui.notation`, and it is the same layer the Python client has:
the score model, the SVG walk and the MEI encoder are the shared Rust core, and
the engraver is [verovio](https://verovio.org) compiled to wasm from the same
pinned sources, with the same options, as the native client links. A page and a
window engrave one score into one drawing — a parity suite compares them
primitive by primitive.

```ts
import { gui, Event } from "clausters";
const { notation } = gui;

// The inverse direction, data → score: the client's own events become MEI.
const melody = [67, 69, 71, 72].map((midinote) => new Event({ midinote, dur: 1.0 }));
const score = await notation.Score.fromNotes(melody, { meter: "4/4", key: "G" });

const page = score.displayList();          // what is drawn, the cursors, the notes
const win = await gui.view({ title: "score" },
    notation.scoreView(page, { name: "page", editable: true })).open();
```

Opening a score is the layer's one asynchronous step, and only because the
engraver is **fetched on demand**: nothing in the runtime imports it, so a page
that draws no notation never downloads it. Everything after that — drawing,
editing, undo — is direct.

An engraved page comes in three layers from one engraving, because the engraver
mints fresh MEI ids on every load and ids from two engravings do not line up:
what the host **draws**, where the **cursor** goes at each onset, and what
**sounds** (one `{t, dur, pitch, id}` per note, which stays in your script — it
is what a driver plays).

Editing rides on those ids. A click reports the element under the cursor; a
vertical drag on a note reports the diatonic staff position the note *reaches* —
absolute, so an edit that arrives twice moves nothing the second time. Only a
note drags: the page says what each id is (`kinds`), and a slur, a time
signature or a rest is selected and never displaced. Your script applies
it and sends the new page back:

```ts
win.widget("page").onEvent((tag, id, position) => {
    if (tag !== "transpose") return;
    score.transposeTo(id, Math.round(position));
    win.widget("page").set({ displayList: notation.pageJson(score.displayList()) });
});
```

`score.undo()` and `score.redo()` are the client's, not the host's — the host
holds no score at all — and what they walk is the **editing context's** pile,
not a stack of the score's own. A page registers in `Editing.of(score)` like a
curve, a take or a roll, and each edit is recorded as the MEI it produced with
the previous one as its inverse, so a window holding a page beside a roll has
one Ctrl+Z, in the order the hand made the edits. `score.canUndo` answers for
that order and may well be an edit to something else.

To **play** the page with the cursor following the sound, `gui.PlayheadSync` is
the same object every time view uses — a multitrack, a piano roll, an engraved page —
and `notation.playheadSync` only fills in the page's unit, since a score places
its cursor in milliseconds where a multitrack places it in samples. It holds no tempo:
beats cross through the map of the timeline a pass plays, or of `structure` when
nothing is playing.

```ts
const tp = notation.playheadSync(host, win.widget("page").id, {
    source: (at) => timelineOf(score).play({ at, destination: server }),
    structure: () => timelineOf(score),
    sampleRate: engine.context.sampleRate,
    extent: () => endOfScore(score),
});
tp.play(server);                 // and pause / stop / locate
setInterval(() => tp.update(), 100);   // parks the cursor when the pass ends
```

The line is the **host's**: `play` sends one anchor — the clock value the view's
time 0 maps to — and the host sweeps from it every frame, so a pass costs one
message and not one per frame. A pause writes the other half of that number, the
static cursor where the music stopped. `update` is the one thing a script owes
it: a pass ends when its last item *starts*, so the transport keeps sweeping
that last note's tail and parks only at the score's extent.

The example is `examples/notation/score-from-data.html`.

### The score behind the page

An engraved page is a picture of a **sheet** — the score as plain data, which
the `notation` module hands you and takes back:

```js
let sheet = notation.sheetFromVoice([{ midis: [60], ticks: 8 }]);
sheet = notation.transpose(sheet, 4);          // up a major third: E
const mei = notation.toMei(sheet);             // ready for engrave/Score
```

A sheet is two structures that do not contain each other: the **grid** (the
metric layout — measures and meter changes, which does not sound and is what
`notation.measures(3, 10)` addresses against) and the **staves** (the content,
flat). Durations are exact fractions of a whole note, `[1, 4]` for a quarter,
and pitches carry their spelling, so transposing sounds right and looks right —
a major third up from C is E, not F-flat.

None of that arithmetic is written in TypeScript: operations are named here and
carried out in the shared core, the same one the Python client and a standalone
host use. `notation.ops()` lists the verbs it knows, and writing a sheet out
throws with a reason when the model holds something MEI cannot say yet — a
tuplet, an accidental past a double, more than one voice.

Every operation is a function from a score to a score, so they compose:

```js
let composed = motif;
for (const section of [notation.invert(motif),        // about its first note
                       notation.retrograde(motif),    // backwards
                       notation.stretch(motif, [2, 1]), // twice as slow
                       notation.transpose(motif, 4)]) { // up a major third
    composed = notation.concat(composed, section);
}
```

`concat` puts one score after another, `stack` puts one against another,
`repeat` plays a stretch several times; `setMeter`, `insertMeasures` and
`removeMeasures` work on the grid. **The two structures move independently**:
`stretch` leaves every barline where it was, so the phrase re-bars across them
and ties where a value overruns one, and `setMeter` rewrites no note. Only the
three that add or remove time move both.

An edit names its item by **id**, never by position — `insert`, `del` (`delete`
is a reserved word here), `silence`, `setDur`, `setPitches`, `tie`, `toVoice`.
`del` and `silence` are different acts: the first takes the item out and what
follows moves earlier, the second leaves a rest and nothing moves.

A sheet holds more than pitches and values: `setMarks` gives one note its
articulations, a dynamic, an ornament and a forced stem, and holds **how long it
sounds** as against how long it is written — kept in the score and not written
onto the page, since an engraver would read it as the real duration and move
every attack after it; `addSpanner` writes a slur or a hairpin,
which has two ends and so lives beside the staves rather than on a note.
`stack` gives several voices on one staff or, with `asStaff`, several staves
under a brace. Tuplets need nothing declared — a duration like `[1, 12]` is
already inside one — though a tuplet that would cross a barline is refused by
name, since it cannot be split. And accidentals are printed only where they are
needed: not where the key signature implies them, not twice in a bar, and a
natural is a sign where the key alters that step.

A page typed as ABC, imported from MusicXML or written by hand is a *document*
and nothing else — none of the verbs above can touch it — until `sheetFromMei`
reads one:

```ts
const score = await notation.Score.open(PHRASE);   // ABC, MusicXML, MEI
let sheet = notation.sheetFromMei(score.mei());
sheet = notation.transpose(sheet, 2);              // every verb applies
```

One input format rather than four: the engraver normalizes whatever it loaded.
**What the model holds is what somebody chose** — the header (`header`,
`setHeader`), the barlines (`setBarline`), the breaks (`setBreak`) and the beams
(a `"beam"` spanner) — because each is a statement, where the beaming and the
line breaks the engraver works out when nobody said anything are recomputed
identically and are not loss. It does not read back the rests the emitter
invents to fill a bar or level a short voice (a score would gain a bar of
silence for having been saved) nor the ids of a foreign document (an id means
something only inside the model that minted it).

**An open score is edited through the same verbs**: `score.sheet()` hands back
the model behind the page and `score.apply(op)` applies one operation as a
single undo step, re-engraving as it goes — so editing a score you have open and
editing a sheet in hand are one operation. Dragging a note is `moveSteps`, which
is not transposition: it moves along the staff and takes the key signature's
alteration for the letter it lands on, so a note dragged onto a B in E flat is a
B flat.

**A page can also be written on**: `scoreView(..., { entry: true })` puts it in
note entry, and a press on a staff reports `"enter" <at> <position> <staff>` —
the element whose column the press fell in. It names a *place*, not a note — a
staff position is a pitch only once something knows the clef and the key — so
the three go straight to `enter(sheet, [1, 4], { item: itemId(at), position,
staff })`, which works the pitch out from that staff's clef and the key and
writes it over what was there, nothing after it moving; `insert` is the act
that adds time.

The way back out is `toNotes`, and it is not a conversion: the symbols mean
something, and honouring them is the whole of the step.

```ts
for (const note of notation.toNotes(sheet)) { /* t, dur, sustain, pitch, amp, staff, voice, id */ }
const timeline = notation.toTimeline(sheet, { instruments: { 0: "piano", 1: "bass" } });
```

Every note comes back with **two lengths**: `dur`, what is written, and
`sustain`, what is heard. A staccato quarter is still a quarter, so the next
attack is where it always was and only the sound is shorter; `toTimeline` puts
the pair straight onto an `Event`'s `dur` and `sustain`. A **dynamic** governs
every note after it until the next one; a **hairpin** is a shape over a stretch
of notes; a **tie** is one sound of the summed length; a note's metric position
stresses it.

**The reading is data, and it is yours.** `notation.interpretation()` hands back
every number it depends on — change what you disagree with and pass it to
`toNotes`; nothing in the core is edited to play a score in another style. What
the defaults claim is as little as a player can claim and still be playing: the
only metric stress is the **downbeat**, because stressing one and three of a 4/4
belongs to a style, and a style passes its own accents.

**What plays a staff is not in the notation**, so each note names the `staff` it
was written on and the binding is made where the score is rendered.

**An event can also say what the note is on a page.** Beside what it sounds,
an event carries the **notation keys** — twelve of them, reserved so none
reaches the synth as a control:

```js
new seq.Event({ midinote: 60, dur: 1, articulations: ["stacc"], dynamic: "mf" });
new seq.Event({ midinote: 63, dur: 1, spelling: "flat", accidental: "written" }); // an E flat, printed
new seq.Event({ pitches: [notation.pitch("f", 4, -1)], dur: 1 / 3, value: [1, 12] }); // an F flat, a triplet eighth
```

Each names one fact, in one unit, and says which way it is read. The table is
the core's, and `seq.NOTATION_KEYS` is its names:

| Key | What it holds | Unit | Which way it is read |
|---|---|---|---|
| `pitches` | a list of written pitches, each `{step, alter, octave}`, with `forced` where the accidental is to be printed | `step` a letter `c` to `b`, `alter` in semitones, `octave` scientific (4 holds middle C) | the source of the pitch on a page. An event that states no `freq`, `midinote` or `degree` sounds the first of them; one with no `pitches` is written from `midinote`, spelled by `spelling` |
| `value` | the written value, numerator and denominator | whole notes, exact: `1/4` a quarter, `3/8` a dotted quarter, `1/12` a triplet eighth | the source of the value on a page. `dur` and `sustain` are its performance in beats; an event without it is written from `dur` |
| `staff` | the staff it is written on | an index from the top, from zero | where it is written; it sounds nothing |
| `voice` | the voice of its staff it is written in | an index from zero | where it is written; a voice is one line |
| `articulations` | a list of articulations | MEI names: `stacc`, `stacciss`, `spicc`, `acc`, `marc`, `ten`, `stress`, `upbow`, `dnbow`, `harm`, `snap`, `open`, `stop` | the mark is the fact; a shortened `sustain` or a raised level beside it is what an interpretation made of it |
| `dynamic` | a dynamic written at this note | its name: `pp`, `p`, `mp`, `mf`, `f`, `ff`, `sf`, `fp` | the mark is the source of the level, and `amp`, `velocity` or `db` its performance |
| `ornament` | an ornament on this note | `trill`, `mordent`, `turn`, `fermata` | the mark is the fact; the notes it is played as are the interpretation's |
| `grace` | that this is a grace note, and of which kind | `acc`, an appoggiatura, or `unacc`, an acciaccatura | on a page it takes no time of the bar and stands before the next note of its voice; its `dur` is how it was played and its `value` what it is written as |
| `stem` | a stem direction the writer forced | `up` or `down` | the page's alone |
| `tie` | that this note ties into the next of its pitch | true or false | the two are one sound on a page; each event keeps its own length |
| `spelling` | which accidental a pitch given as a number is written with | `sharp` or `flat` | a preference, read where the event has no `pitches` and where a moved sounding pitch spells them again |
| `accidental` | whether the accidental is printed where the key or the measure implies it | `written` or `sounding` | read only where the event has no `pitches`, whose own `forced` says it |

Every one is a **musical fact rather than an instruction to the engraver**
(`articulations: ["stacc"]`, never "draw a dot"), which is what lets the same key
be read in both directions. An explicit `sustain` becomes how long the note is
*held*, but only where no symbol already says it — a note that is both staccato
and short is one fact, not two, and writing both would shorten it twice on the
next reading.

Two of them are the written form of something the event also sounds. `pitches`
is the note as it is written — a letter, an alteration, an octave, as
`notation.pitch` makes it — where `midinote` is a number two written notes
share: no spelling of 64 is an F flat. An event that states only `pitches`
sounds it; one that states a sounding pitch beside it sounds that, and the page
still writes `pitches`. The two move together: setting a sounding key on an
event that holds `pitches` spells them again by `spelling` (a move by octaves
keeps their letters), and setting `pitches` moves what it sounds. A chord is one event per note, each with its own pitch,
as it sounds. `value` is the written value as an exact fraction of a whole note,
where `dur` is time in beats: a triplet eighth is a third of a beat that no
float holds, and a grace note has a value and takes no time of the bar at all.

**What is not any note's is not a key.** A slur and a hairpin have two ends; the
meter and its changes, the key, the clefs, the breaks and the page's text belong
to the score. A sequence rendered from a score keeps them in its `notation`
section, beside its tempo map, where what has two ends names them by event id.

**The round trip is honest, and both directions lose something.** Events to a
score loses exact onsets, continuous amplitude, microtones and the instrument;
coming back, a note keeps everything written *on* it and loses everything that is
not one note's — a slur and a hairpin have two ends, a meter change and a barline
belong to the grid, a title belongs to the document. What survives both ways is
the note: its pitch and spelling, its written value, its marks, and their order.

**A gesture names an element; a verb names an item**, and `notation.itemId` is
the step between them. The page reports the element under the cursor the way the
emitter spelled it — `n7` is the item, `n7-2` a part of it split across a
barline, `n7-p1` one pitch of a chord — and all three are item 7, which is what
lets a gesture anywhere on a note reach the note. It comes from the core because
the answer is the *emitter's*: a client reading the ids itself would disagree the
first time a split was spelled differently.

`examples/notation/score-editor.html` operates on a document this way, opens it
in the score editor, edits it through the editor's verbs, and plays it.

## Bindings, and the page that runs without a script

A widget's value can bypass this script entirely — to the audio server, or to
another widget:

```ts
win.widget("cutoff").bind("/node_set", 1000, "freq");
win.widget("picker").bindWidget(win.widget("pages"), "index");
```

Both matter more here than on the desktop, because a **bundle** is a GuiDef
whose widgets are bound and whose boot list starts its graph: the page mounts
it and no client library runs afterwards. Writing one is the Python client's
job ([bundles](https://clausters-python.readthedocs.io/en/latest/bundles.html));
mounting one is [Components](components.md).

## Reference

- Every builder, its options and its event payloads: the [API
  reference](api/Namespace.gui.md).
- The wire itself — the `/gui_*` commands, the axis properties, the edit-back
  payloads: the server guide's [GUI protocol
  chapter](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
- Reading buses and buffers *from the script*, to draw with your own code
  instead of a widget: [Reading the server](data.md).
