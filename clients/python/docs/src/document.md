# The document: what a multitrack is, and who edits it

A `Timeline` places items at beats and plays them. That is enough to
sequence, but not enough to *compose*: a multitrack places recorded and
generated contents in time, with tracks, takes and curves that are **authored,
durable and undoable** — state
a picture cannot hold, because a picture is drawn and drawing frees.

That state lives in the **document** (`crates/clausters-document`), and this
chapter is about it: the model a multitrack editor edits, the presentation beside
it, and how an edit is applied, inverted and saved. It is one crate, bound by
every client and by the `standalone` host, so what an edit *means* is defined
once rather than re-derived per language.

**The data is what is fundamental.** Samples, notes, events, curves — they are
edited and drawn with no arrangement anywhere near them: `edit(x)` opens a
buffer, a timeline or a curve on its own. So the pictures are independent of the
model too — **a clip is a view configured by what it holds**, and the edits it
admits (move, trim, split, join) come from the structure inside it, in the unit
that structure measures. A clip over samples and a clip over a timeline of notes
take the same actions; only the arithmetic differs.

`clausters.form` is a **relegated** client-side layer for placing elements in
time, kept frozen and taking no new work; it has no view and nothing is designed
around it. See [`clausters.form`](form.md).

## What an editor is

`clausters.gui.editing.Editor` edits **one structure**, a break-point curve or a
timeline of events, and it knows nothing about any
arrangement. That is the whole of it, and it is deliberately the plain case:
editing a curve is what an editor is for.

An editor orchestrates rather than performs, and it is four collaborators
(`clausters.gui.editing`):

| | what it is | what it deliberately is not |
|---|---|---|
| `View` | the picture of one structure, and the registry from widget id to what it shows | not the vocabulary: one structure is drawn several ways |
| `Domain` | what a gesture needs read with it, and the applied payload written onto the client object | not **what a gesture means** and not **how an edit inverts** — both are the shared crate's, so neither is written once per language — and it does not draw |
| `Echo` | the acknowledgement: the stamp, the version, the corrections, the reason | not anything about what was edited |
| `Editing` | the editing context: the history, and the views to tell | **not the editor's** — it is asked for, never built, which is what makes two windows walk one undo order |

The rule that fixes all four: an editor owns **neither the data nor the
history**. `View` here is not `clausters.gui.guidef.View`, which is a tree you
can open.

## `edit(x)`: one verb over the four structures

`clausters.gui.edit` opens whichever editor the structure asks for, and it
dispatches on **what the structure holds**, never on its class — a curve editor
asks for break points it can read and write back, and everything that answers
opens, which is why the curve row below names three unrelated types:

| `edit(x)` where x is | opens | over | its vocabulary |
|---|---|---|---|
| a `SoundFile` | `AudioEditor`, the handler of the GUI host's audio editor | a `waveform` | `parts` |
| a saved `multitrack.Session` | `MultitrackEditor`, the handler of the GUI host's multitrack | a `multitrack` | `clips`/`tracks` |
| a curve — a `Bpf`, an `Env`, a `multitrack.Automation` | `PointsEditor` | a `bpf` | `points` |
| an `EventSequence`, or a `Timeline` rendered into one | `NotesEditor` | a `pianoroll` | `events` |
| a `Multitrack` | `MultitrackEditor`, over an untitled session | a `multitrack` | `clips`/`tracks` |
| a `Score` (a symbolic score) | `ScoreEditor` | a `score` page | `score` |

**An `EventSequence` opens in the notes editor**, `clausters.gui.editing.NotesEditor`, and is edited **in place**: the editor is the shared crate's, and it holds the very sequence the script's handle names, so there is nothing to write back. Every note on the roll carries the id of its event, so a drag, a trim or a velocity changed with Shift and a vertical drag names the note it touched and keeps everything the roll cannot draw -- its instrument, its amplitude. **A `Timeline` is rendered first** (`Timeline.render_events`): the roll edits the events it produced, which are the editor's `sequence`, and the timeline, being code, is left as it was.

**It plays what it edits**, on its sequence's own transport, so playing it never moves a multitrack nor another roll's sequence: `editor.play()` (from the position cursor, or a beat given), `pause()`, `resume()`, `stop()`, and the space bar over the window, which is play/stop -- a stop goes back to the position cursor. **Where a pass ends** is `editor.end`: `None` by default, the transport rolling on past the last note as a multitrack's does; `"contents"`, where the last note ends; or a beat, an end marker. A note's release rings out past a stop, which releases the notes rather than freezing them. An edit while it plays is heard at once -- the transport's event lane takes the sequence again and plays on from where the position is -- and a note already sounding ends as it would have. The roll's play cursor is that transport's position, stopped or rolling, and a click on its ruler places the **position cursor**: a stopped transport is cued there, and the next play starts from it. **The roll's vertical axis can be a frequency**: `y_axis="hz"`, on `NotesEditor` or through `edit`, draws the same notes on a log scale ruled in hertz, where a note an octave up is as high as on the keys and a drag moves it continuously, writing its `freq` -- the MIDI note it was written with follows it. Two editors over one sequence, one in each axis, are two windows onto one history. **A script edits the same sequence while the roll is open**, through its objects (see [Event sequences](timelines.md#event-sequences-events-as-data)): the change is a turn of that history, so the roll redraws, the lane plays it, and Ctrl+Z in the window takes it back — `with seq.history("humanize"):` makes a whole loop one entry. **What the hand marked is the script's to read**: `roll.selected` is the events marked on the roll (a click, Shift+click or a marquee) as the `SeqEvent` objects they are, `roll.select(events)` marks them from the script and `roll.unselect()` clears — the picture's state, entering no history. The other selection, the time range an Alt+drag sweeps, is the transport's `span`, and `roll.transport` is that transport (see [The transport](transport.md)). `play(destination=midi)` sends the events to a MIDI port instead, on the client's clock, since the server has no MIDI output.

**A roll can paint what is played**: `edit(sequence, midi_in=True)` asks the host to open its MIDI input and write each incoming note into the roll — on the step cursor while the transport is stopped, at the play line while it plays, held as long as the key. Each is an edit of the sequence like a note a hand drew, so it is in the history and in every other window over it. A native host opens a port named `clausters-gui` to route a keyboard into.


**A saved session opens in the multitrack**, an application the GUI host holds,
over the session's file: `edit(SavedSession.open(path))` (a session saved with
`session.save(path)` has a file too; one with none is refused). The session's
takes are read where they are, never loaded whole: their headers as it opens,
their peaks as the window draws them -- kept in the session's own `peaks/`
folder, beside nothing of yours, and measured once when they are missing -- and
their samples as the transport reads each track's regions, summed where they
overlap. A box of notes plays the sequence the session holds, and a double
click on one opens its roll in the same application, one history with the
multitrack. A `Multitrack` opens the same way, as an **untitled session** over
it: no file behind it until the first `save(path)` names one. What comes back,
`clausters.gui.editing.MultitrackEditor`, is its handler:
`transport.play(start, end, looping=True)`, `pause`, `stop(mark)`,
`locate(at)`, `playing`; `undo`, `redo`, `split`, `join`, `quantize`,
`stop_at_end`, `reset_heights`, `compact_tracks` and any window verb by name
(`verb`); `selected`, the boxes the hand holds as the multitrack's `Region`
objects, `select(regions)` and `unselect()`, which are the window's and enter
no history; `grid`, `snap_to_grid` and `notes_view`; `output`, each track's meter
and the samples played as silence (`starved`); and `save(path=None)`, which
writes the session as the edits have left it, every file inside its folder
named relative to it. The window saves too: its Save writes
where the session saves, and an untitled one asks where in the Save as form,
as Save as always does. **The script's multitrack is the window's while it is
open**: the session's `multitrack`, or the one handed over, reads what the
window shows now, a change made through its objects is an edit of the window's
that its Ctrl+Z takes back, and `multitrack.history` is the window's history
(`with multitrack.history("tidy"):` makes a block one entry). When the window
goes, the multitrack holds what the window left it as.

**A `SoundFile` opens in the audio editor**, an application the GUI host holds: the host
opens it in a window of its own, on an engine of the application's own, and every gesture,
render and undo runs in the host's process. What `edit` answers is its **handler**,
`clausters.gui.editing.AudioEditor`: each of its members is a call to the application
(`GuiHost.open_app`, `GuiHost.call_app`), and what it reads is asked again each time. A
server buffer is not edited, since it is loaded on a server the application does not
reach: `buffer.write(path)` and edit the file. It writes nothing it was handed while it edits. The take is **a list of spans of files**, and
every edit leaves a new list: a cut takes a span out, a paste of a copy puts its spans in,
and an edit that makes new samples -- a pencil stroke, a pasted block, a mix
(Ctrl+Shift+V), a gain curve applied, Process -> Reverse -- **renders** them offline into a
file of the editor's working folder, which the list then names: a stroke is a file the
size of the stroke, spliced over the frames it was drawn on. An undo is the list before,
so it costs the list and not the samples, and a take of an hour cuts and pastes as fast as
one of a second. The files a history or the clipboard can still reach are kept, and
deleted when nothing can. **A file changed on disk is said**: when the window comes back, the files the take reads are looked at, and one another program changed or removed since the editor read it is said on the window's status bar; the editor's own save is no change. The working folder is the host's, a folder of the user's cache on disk
(`clausters/work` under the platform's cache folder, or `$CLAUSTERS_CACHE_DIR`), and goes with
the window; one a host left when its process was killed goes the next time any editor opens. A
render that fails takes away what it had written there. A WAV, RF64 or W64 file is read where it
is; any other (an MP3, a FLAC) is decoded into the working folder a run at a time when it
opens, and the take appears when the decode ends. `reads_in_place(file)` says which. The
decoded copy is what is edited and the file opened is never written over: its first save asks
for a path, and writes a WAV. `editor.save()` renders the edited take over the file it was
opened from (moved aside first while the list reads it, and the render moved where it was),
answered once the file is whole, and Ctrl+S in the window does the same. `editor.save(path)`
renders it as another file, which a later save then writes over. `edit(SoundFile.empty(channels, frames, rate))` is silence not
written anywhere yet -- its first save needs a path, which Ctrl+S asks for in a **Save
as...** form and `editor.save()` refuses without one. `editor.file` is the `SoundFile` the
take is now. The window's File menu has **New file...** (a form for the channels, the
length and the rate, which adds the new file to this editor), **Open file...** (the host's
file chooser, with Ctrl/Shift to select several files) and **Save as...**. `editor.parts` is
the spans of files the take is made of. **It plays what it edits** on the application's engine,
on a transport of its own (another window's play moves nothing in it), and `editor.transport`
is that transport in the take's seconds, with the verbs and the span a server's `Transport` has: the space bar over the window plays the
marked range, or from the position cursor, and `L` loops it -- the same
`transport.play()`, `span` and `loop(start, end)` from a script, each side
reading what the other set. **A range swept while it loops is the loop's new
span**: the loop follows the hand as it draws, and the head goes to the start
of the range when the sweep is let go. A pass that is not looping keeps the end
it was played with, and the next play reads the range. What the hand marked is
samples, and
`editor.selected` answers them as a `Segment` over `editor.file` (its `start`
a frame, its `duration` seconds); `editor.select(segment)` marks one and
`editor.unselect()` marks nothing. In an audio editor the samples marked and the
transport's `span` are one range, so setting either sets both.

An editor holds several files, listed in an independent widget on the left.
`editor.files` lists the open SoundFiles,
and `editor.file` is the focused one. `editor.open_file(path)` or
`editor.open_file([path_a, path_b])` adds files to the same editor;
`editor.new_file(channels, frames, rate)` adds an empty one.
`editor.focus_file(index)` and `editor.close_file(index)` use zero-based
list indices; omitting the close index closes the focused file. Each file
keeps its cursor, selection, edits and undo/redo history. Switching files
restores its cursor on the same transport; only the focused file plays. View -> Files toggles the list;
`editor.hide_files()` and `editor.show_files()` do the same from a script.

**The spectrogram's settings** are in a panel to the right of the take,
which View -> Spectrogram settings shows and hides (as
`editor.show_spectrogram_settings()` and `editor.hide_spectrogram_settings()`
do): the analysis window's size and shape and how much one window overlaps
the next, the frequency scale (a note axis at a reference A4 among them),
the colours, and whether each band is drawn at its own scale.
`editor.spectrogram` reads them and `editor.set_spectrogram(window=2048,
scale="note", tuning=432.0)` changes the ones it names. They are the
window's whichever file is in focus, and a view state outside the history.

**The editor opens without a file.** `AudioEditor().open()` (from
`clausters.gui.editing`) is the application on its own: its window keeps the
menu bar and the file list, says where the take would be that no file is
open, and offers File -> New file and File -> Open file. Closing the last file
leaves an editor in the same state rather than closing it; File -> Close
closes the window. With no file open, `editor.file` is `None`, `editor.files`
is empty, and a verb that acts on a file (`parts`, `save`, an edit) raises
`ValueError`.

The audio editor's View menu selects waveform (`W`), spectrogram (`S`) or both (`B`). The same actions are `editor.view_wave()`, `editor.view_spectrogram()` and `editor.view_both()`. `editor.layers` accepts names (`peak`, `rms`, `signal`, `momentary`, `short`, `spectrogram`) or objects with `draw`, `visible`, `solo`, `alpha` and `y`. `show_layer(name)` and `hide_layer(name)` preserve a layer's place and settings; `solo_layer(name)` toggles solo, and `layer_alpha(name, alpha)` sets its opacity in `[0, 1]`. Waveform and spectrogram are two **views** of the take: one replaces the other in the take's lane, and both (`B`) stacks them, the waveform in the take's lane and the spectrogram in a lane under it, on one time axis with one selection and play cursor and a vertical each. The layers of `editor.layers` are the **visualization layers** of the view in the take's lane; a frequency layer and an amplitude layer cannot both claim its vertical, and a stack can still superpose them with one of the two in boxes of its own. The menu's views are the defaults, not the only ones. The gain envelope is an **edit layer** over the take, superposed on whichever view is in its lane. Changing visualization leaves the take and its history intact. The vertical ruler is in the units of the layer that claims the axis, and each quantity keeps its own vertical zoom: a spectrogram opens no further than its whole frequency range, and returning to the waveform finds the amplitude zoom where it was left.

The picture now stands in a `pane`: one time ruler, selection and playhead
shared by its bodies. `editor.layer = "take"` puts the take body in hand, the
same action as View → Edit audio. This edit body is separate from
`editor.layers`, which chooses pictures of the samples. `pane(...)` also
accepts curve bodies; its `layer` property names the active body (`points`,
`points:1`, etc.), and `hidden` excludes named bodies from drawing and editing.

`editor.show_envelope()` (Process → Edit gain envelope) shows and selects the gain
curve. `editor.envelope` reads its flat `time, value, shape, curve` quads in
seconds; `editor.set_envelope([...])` records a curve edit in the same undo
order as sample edits. The header names the gain scale: zero to one, where one is unity, and the
vertical ruler and the read-out under the pointer measure gain while the curve
is in hand and the samples while the take is. The
segment takes its shape and curvature from its first point, using the same
shapes as an envelope. Values before the first point and after the last hold.
The curve stays on the take's time axis when sample spans are cut or pasted.

`editor.apply_envelope()` (Process → Apply gain envelope) renders that gain into
a new take and resets the curve to unity. Undo restores both the original
take and the unapplied curve; redo restores the rendered take without applying
gain a second time. Curve edits alone leave the samples unchanged.

The curve is heard once it is applied. Save writes only the current samples;
apply before saving to keep the gain in the file.

```python
from clausters.gui import edit

editor = edit(curve, sample_rate=48_000.0)

curve.to_points()      # the edited curve, out of the object you already held
```

**It opens.** The window is up and listening when that returns, so the
structure you already hold is the edited one from that moment: read it whenever,
and it says what the hand has left there. `open=False` builds the editor without
a window, for a caller composing one.

Nothing is handed back: the object passed in *is* the edited one, except a sound file, which the audio editor writes when it is saved. A
**multitrack** is one of them, and what opens is the multitrack editor — the
rest of this chapter is what it edits.

Reading it back **after the hand is done** is `wait`:

```python
editor.wait()          # returns when the window is closed
curve.to_points()      # the curve, as it was left
```

`editor.closed` is the same question asked without waiting, and
`editor.close()` closes the window from the script — the history is not closed
with it, since an undo order belongs to the data.

**Two calls over one structure give two windows and one stack.** The editing
context belongs to the data, so an undo in either window steps the one order both
of them made. And a window composing several structures passes one context
(`edit(x, context=...)`), which is what makes it undo across a curve and a roll
in the order the edits happened.

**How an edit inverts is the shared crate's.** For a curve and a timeline the
state goes in with the payload and comes back as what the structure now is *plus*
what puts it back — one call, because the inverse has to be read before the edit
lands. A span of samples is the exception, and a real one rather than an
omission: the frames are in a server buffer, so the crate holds no state to
invert. What it shares there is the payload's shape and its coalesce key, and the
inverse rides on the wire — a stroke's event carries the run it wrote *and* the
run it replaced. This client writes a stroke's samples synchronously; a page's
buffer calls are asynchronous, so the web client queues them in order instead,
and that is the only difference between the two.

## A running node: `edit(node)`

A `Synth` — of a SynthDef, a FaustDef or a plugin — is not a document, and
`edit` opens it all the same: a window of its def's controls that nobody
wrote, `clausters.gui.editing.NodeEditor`.

```python
freq = control("freq", 660.0, min=220.0, max=1760.0, warp="exp", unit="Hz")
node = Synth("tone")          # of a def with that control
editor = edit(node)
```

The editor reads the def's controls as `/def_query` reports them — the same
record in every family — and the shared core decides what each becomes: a
slider over the range it declares, a toggle for a control of two steps, a list
for a plugin parameter whose steps have names, a number with arrows for one of
more steps than a groove holds. A control that declares no range is drawn over
the range its name conventionally has (`freq`, `amp`, `pan`, …), or else around
its default. Hidden and read-only parameters, and controls fixed when the node
starts (`"ir"`), are left out. Beside each control is its value as the server
writes it: the number with its unit, or a plugin's own words for it (`-6 dB`,
`Hall`).

**The host keeps the window, not the script.** Every control is
[bound](gui.md#values-that-never-come-back-to-the-script) to the node, so a drag
reaches the server with no round trip through Python, and every binding is read
back, so the window follows the node whoever changes it. A plugin of thousands
of parameters opens by groups (its modules) and pages of sixteen, with a search
field over the names; `editor.search("cutoff")` and `editor.show_page(2)` do the
same from the script, and `editor.window["control:freq"]` is the widget of one
control (`"text:freq"` its text).

A running node's controls are the server's state, so nothing here enters an
undo order. A plugin's **own** window is something else: it opens on the
server's machine with `node.open_plugin_editor()`, and the two can be open at
once, each following the other. To read or set a value as text from a script,
`node.get_text("freq")` and `node.set_text({"freq": "880 Hz"})`.

## The score editor: `edit(score)`

**A `Score` opens in the score editor**, `clausters.gui.editing.ScoreEditor`, and is edited **in
place**: the editor is the shared crate's and holds the very score the
script's handle names, so `score.sheet()` and `score.mei()` read every
edit. `edit(score)` answers the editor, as it does for every structure. A score
read from a document somebody else wrote is written from its model when the
editor opens, so a press names an item of the model at once.

### The page: selecting, moving, writing

The window's page is the engraved score. A click selects a note and the status
line under the page says what it is; a drag moves it along its staff — only a
note drags; a slur, a time signature or a rest is selected and never displaced
— and a press on a staff selects the measure it fell in.

**Several notes are selected at once**: Ctrl+click adds a note or takes it out,
and Shift+click extends the selection to the note clicked, in time and across
the staves between. `editor.selected` is what is selected, as the model's item
ids, and `editor.select(elements)` selects from the script by the page's
`xml:id`s.

**Everything the page draws can be picked, and a verb means what the selection
makes it mean** — the rule the field's editors share. A press on a slur, a
hairpin, a dynamic, an ornament or a tempo mark selects *it*, and the status
line says what it is in the model; Delete then takes away the slur and not its
notes, the dynamic and not the note it is under. A verb that asks for notes
reads the notes the selection is attached to: with a dynamic selected, another
one from the palette replaces it, and with a slur selected a hairpin is written
over the same notes. The page names each after the model — `a-slur-2-11` is the
slur from item 2 to item 11, `a-dynamic-n3` the dynamic of item 3 — so a line
drawn in two parts, across a page break, is one thing to pick.

**The keys act on the selection** where the window is not writing notes: Delete
or Backspace takes it away, Up and Down move the selected notes a step and Ctrl
with them an octave, Left and Right move the selection to the item beside it,
and Escape lets it go. They are the keys note entry reads, meaning what the mode
makes them mean, and bindings of the key table under `[gui.keys.score]`.

**Writing is a mode, note entry**, and the window opens outside it: N, the
toolbar's pencil or `editor.entry = True` enter it, and Escape, N again or a
play leave it. In it an **edit cursor** stands on a staff, in a voice, at a time
— on what was selected, or where it was last left, or at the first beat — and
what is entered is written there **over what was there**: nothing after it
moves, in the bar or past it, and a value longer than what is left of the bar
goes on into the next one, tied. A letter `a` to `g` writes that pitch, in the
octave nearest the note before it, of the value in hand (`editor.value = (1, 8)`
writes eighths; the digits 1 to 7 pick a value, from a 64th to a whole, `.` the
dot and `0` a rest), and the cursor goes on by it; Shift and a letter add the
pitch to the chord just written. The arrows move the cursor a note at a time,
Ctrl and an arrow a bar, Alt and Up or Down to the next staff, and Ctrl+Alt+1 to
4 put it in that voice — which is how a second voice is written against the
first; Up and Down move the note just written a step, Ctrl and them an octave. A
press on a staff writes at the time it fell at, or, on a note of the cursor's
voice, adds the pitch pressed to its chord. Every one of these keys is a binding
of the host's key table, under `[gui.keys.note_entry]`, read before the
selection's own under `[gui.keys.score]`, where `N` is.

### The window: a menu bar, a toolbar, palettes and dialogs

Everything the window holds is the application's, and every piece of it is one
of the editor's verbs, so a pick and a method call are one path.

**The menu bar holds every action** — File (open, save, export, the paper and
the page's text), Edit, View (page or continuous), Play, Notes (whether a press
writes, the value it writes, a step or an octave, a length, a tie, the other
voice), Notation (slur, crescendo and diminuendo, dynamics, articulations,
ornaments), Measures (insert and remove, the meter, the barline, a system or a
page break) and Transform (transpose, invert, retrograde, augment, diminish,
repeat).

**The toolbar holds what a hand reaches for while it writes.** Each tool is
drawn with the engraver's own symbol — the note of that value, the accidental,
the articulation — whose outline the editor asks the engraver for and the
window carries, so the toolbar shows the font the page is engraved in; what
that face does not hold — a slur and a tie, a hairpin, a barline, a grace note,
a break — the editor draws itself, in the same form, so no tool and no entry is
a word. A symbol is drawn at nearly twice the size of the words beside it, the
proportion an icon has to a caption. Three
tools are the *input state*, what the next item written on the page takes — the
value, its dot (`dotted`) and whether it is a rest (`rest`) — and the
accidental is both: with notes selected it is theirs (`accidental(alter)`), and
with none it is armed for the next note (`next_accidental`) and let go
once that note is written. The others act on the selection: the common
articulations, a tie, a triplet, and the voice (`voice(to)`; the tool shows the
voice of what is selected). At the far edge is the transport.

**Beside the page stand the palettes**: what can be written on a score, a kind
of element to a group that folds on its title — notes (a triplet, the two grace
notes, which is the `grace` verb, the other voice, the tremolos on one note and
between two, a beat repeat), accidentals, the articulations by their MEI names,
ornaments (and a rolled chord, a glissando, a breath and a caesura), lines (a
slur, a tie, the hairpins, let it ring, a phrase mark, the octave lines, the
pedal, a bracket, a beam across a barline), text (a tempo mark, a direction, a
rehearsal mark, a fingering, a chord symbol, a syllable of the lyrics),
dynamics, measures (insert and remove, the barlines, the breaks, the endings,
segno and coda, fine, da capo and dal segno, a measure repeat, and empty
measures drawn as one numbered rest), keys and clefs changed from the
selection on, and staves (one line or five, a name, a transposition, a brace,
a bracket or a line over the selected staves). An entry is drawn with the
engraver's symbol, says what the element is when the pointer rests on it, and
is one of the editor's verbs over what is selected; one that needs words opens
a field for them. Only what the score's model holds is offered, and the column
scrolls when its open groups are taller than the window.

**What is written is also what is heard.** The repeats, the endings and the
jumps are played out in order, a tempo mark is the tempo from where it is
heard, the pedal is the sustain controller, an octave line and a transposing
staff move the pitch, a tremolo is its repeated notes, a rolled chord is
staggered, a glissando moves the pitch continuously to the next note, a breath lets
the note go early and let it ring holds it — and the page's cursor follows the
same reading.

**An entry that ends in three dots opens a dialog over the window**: the page's
text (every field of the header, and the footnotes one to a line), the page's
margins and the staff's height in millimetres, the number a transformation
takes — the semitones of a transposition, the factor of a stretch, how many
times to repeat — and a file's path. Nothing is written until OK, which is then
one step of the history however many fields changed; a field that cannot be
read keeps the dialog up and the status bar says why.

### Without the chrome: the page and the handle

The chrome — the menu bar, the toolbar, the palettes, the dialogs — is how a hand reaches what the editor does when nothing else can: a
standalone host opens the application with nobody beside it, so there the
window is whole, always. A script has the handle, and can open the same editor
with none of it:

```python
editor = edit(score, chrome=False)
editor.on_change = lambda: process(score.render_events())
```

`chrome=False` is the page in its scroll, over the status line. Nothing else
is composed, so nothing else is engraved for, sent or kept current: the window
of the example is four widgets where the whole application is over two hundred, and
a quarter of the bytes. **The keys are not chrome and stay** — `N` and note
entry, the arrows and Delete over the selection, the space bar, Ctrl+Z —
since a key is a binding of the window and not an entry of a menu, and F1
lists every one of them on a sheet over the window. Every verb of the handle
stays too: one refused answers so, and the status line says why. What a
dialog asked is asked of the handle: a score with no file yet is saved with
`editor.save(path)`, and the window's close mark closes it at once, since the
score is the one the handle holds.

What a hand writes there is read where everything else is, on the score the
script holds: `score.sheet()` is its model and `score.render_events()` the sequence it
renders into, and `on_change` is told once per gesture, whoever made it. The
option is every editor's (`edit(x, chrome=False)`); the score editor is the one
application with chrome today, so an editor with none opens the same either
way.

### The verbs, over what is selected

`move`, `scale`, `articulation`, `dynamic`, `ornament`, `grace`,
`accidental`, `clear_marks`, `tie`, `silence`, `delete`, `voice`, `mark` (a
tremolo, a roll, a breath, let it ring, a fingering, a chord symbol), `lyric`,
`beat_repeat`, `control` (a tempo mark, a direction, a rehearsal mark),
`set_clef` and `spanner` (from the first selected note to the last) act on
what is selected, and `operate` applies any model operation. The measure
verbs — `insert_measures`, `remove_measures`, `set_barline`, `set_break`,
`set_meter`, `set_key`, `set_ending`, `navigation` and
`measure_repeat` — act on the measures the selection covers, `set_staff` and
`group` on its staves, and `multirests` on the whole score. Each is one call
into the crate and one entry of the editing context's history, so Ctrl+Z over
the window walks it back.

**A transformation takes the measures the selection covers**, or everything
with nothing selected: `editor.transform("transpose", semitones=12)`, and likewise `"invert"`,
`"retrograde"`, `"stretch"` and `"repeat"`.

### The paper, the layout and the page's text

**The paper is the score's and the layout is the window's.** The score is drawn
on its paper — A4 until somebody chooses — as fixed pages one under another: a
window resize shows more or less of them and never re-flows the music, a drag
on blank paper pans and Ctrl with the wheel zooms. `editor.set_page("Letter", landscape=True, staff=800)` lays it
out on another (a paper by name — `editor.page["papers"]` lists them — or a `width` and
`height` of its own, the `margins`, the `staff` height; tenths of a millimetre,
the staff in hundredths), as an edit like any other: it is undone with the rest
and written into the MEI, so the score opens on that paper tomorrow.
`editor.layout = "continuous"` looks at the same score as one system as long as
the music, with no page, and `"page"` goes back; that edits nothing.

**The page has text of its own**: the title and its subtitle, the composer and
the arranger, the lyricist and the translator, a copyright line and footnotes
are fields of the score, each written in a cell of the page's head or foot —
three columns by three rows, on the first page or on every page. Nobody having
moved it, a field sits where the printed page puts it: the title centred with
its subtitle under it, the words' authors on the left and the music's on the
right, the copyright centred at the foot of the first page, the footnotes at
the foot on the left; the page number is written from the second page on, at
the top outer corner: an even page's on the left, an odd one's on the right.
`editor.set_text("title", "Eight bars")` writes one, `editor.set_text("note", "* a footnote")` adds a footnote, and
`editor.set_text("composer", halign="left", pages="all")` moves one — each an entry of the history. A click on a text
says which field it is, and where it is written, on the status line, and **a
double click types over it where it is drawn**: the whole of it selected, so
the first key replaces it; Enter, or going elsewhere, writes it as one entry
of the history, and Escape leaves it as it was.

### Playing the score

**The score plays on the server, and the page's cursor follows it.** The space
bar over the window, the toolbar's transport (back to the start, play, the loop
switch) and the Play menu play the score on a transport of its own —
`editor.play()`, `editor.stop()`, `editor.playing`, and `editor.transport` for the
transport itself, whose beats are quarters. **A selection is a stretch or a place**, as a
swept range and the position cursor are in the other editors: a measure —
a press on its staff — a selection extended with Shift, several notes, or
everything, is played from its first note to the end of the one that ends last,
once, or over and over with the loop on (`L`, the toolbar's switch and the
menu's are one switch); one note picked is where a play starts, and with
nothing selected it starts at the cursor and the loop repeats the whole score.
What plays is the score rendered into a
sequence, at the engraver's own time of 120 quarters a minute, which is the
time the page's cursor is drawn over; an edit made while it plays — and an undo
— is heard on from where the position is. The cursor is the transport's
position, which the host reads from the server it is a client of: the session's
own host, opened with `session.gui()`, is one, and with no host open the editor
has one booted as a client of the server it plays on. A host named by hand
that is a client of no server still edits the score and draws no cursor.

### The score's file

**A score is saved as itself.** `score.write(path)` writes it to a file as MEI —
notes, marks, slurs and hairpins, the meter and the barlines, the page setup
and the page's text, not a render of any of it — and `notation.Score.read(path)` reads one
back as the same page; it also reads MusicXML, ABC and Plaine & Easie,
whichever the engraver finds the file to be. A path names the disk. The
score remembers its file (`score.path`), which is what the File menu's **Save**
and Ctrl+S write to; a score with none is asked for one, as **Save as...**
always asks, and **Open...** reads another document in place of the score, as
one step of the history. The same two are `editor.save()` and
`editor.load(path)`. **New** puts an empty score in its place the same way —
one staff in the treble clef, common time, C major, four empty bars, and no
file yet — and **Close** closes the window, asking first when the score has
changes its file does not hold (`editor.unsaved`): save, don't save, or cancel.

**A score is exported as what it renders.** The File menu's **Export MIDI...**
and **Export clip...** — `editor.export(path)`, which takes the format from the
extension or as `"smf"` or `"clip"` — write the rendered sequence as a Standard
MIDI File or a MIDI 2.0 Clip File: the notes, a channel to a voice, the
dynamics as each channel's expression, the pedal on controller 64, at the
score's own tempo. It is the same
writing a sequence does (`to_smf`, `to_clip`), so what is
exported is what the roll shows.

### The score into a sequence, one way

`score.render_events()` is the verb a timeline and a pattern have: it answers an
`EventSequence`, which is what plays and what a roll edits —
`edit(score.render_events())` is the score as notes with their automation — and nothing
made from that sequence travels back to the score.

Every sounding note is an event, a chord one event per note, and each still
says what it is on the page (`clausters.seq.event.NOTATION_KEYS`): the pitch as it is written, its
value, its staff and voice, its marks. A voice renders on a channel of its own.
A staff's dynamics and hairpins are a curve of its channels as well as each
note's attack; on a staff with two voices the same curve is on both channels,
named as one group. A hairpin is straight in amplitude from its first note's
onset to its last one's, and one that ends on no dynamic holds the level it
reached until the next one. A note held under it carries the hairpin as its
own curve over `amp` too, so a crescendo moves inside a held note on a synth,
which listens to no controller. Which of the two an instrument hears is the
reading's — a struck sound hears the attack alone, a sustained one the curve
as well: `interp={"dynamics_as": "attack"}` writes no curve and no
note carries the hairpin, `"curve"` leaves the attacks unmarked, and `dynamics_cc` names the controller
(11, expression). What is no note's — the meter, the key, the clefs, the page
and its text, a slur — is kept in the sequence's `notation` section.

**Which MIDI specification the sequence is written for** follows from what its
notes carry, by one rule. A curve is a channel's or one note's, whatever the
specification, and the specification is what says the second. So a score whose
notes carry no curve of their own renders as MIDI 1.0, a channel to a voice.
One with a glissando — a note bending on its own — is MPE where the score is
one line, since there each note takes a channel and the line's own curves the
master's; and MIDI 2.0 where it is several voices, which says a note's curve
and a channel's side by side.

### A sequence as plain data, and in parts

What the editor made is in the events, and a script often wants it back as
numbers of its own. `to_rows` chooses the keys and answers a list of tuples in
that order:

```python
seq = score.render_events()
seq.to_rows("midinote", "dur")        # [(60.0, 1.0), (62.0, 0.5), ...]
seq.to_rows("at", "pitches", "value") # where, and as written
```

A key is read as the event means it, not as it happens to spell it: a note
written by its `degree`, its `freq` or only its `pitches` has a `midinote`, and
one that states a `velocity` has an `amp`. `"at"` is the event's beat; a key
the event does not hold is `None`. That is one row per event, the sequence as
it is placed.

`line=True` answers the other shape, **one line played back to back**, the way
a pattern writes one: notes that start together are one row whose differing
keys hold lists (a chord), `dur` is the time to the next row, and a silence is
a row of its own, every key `None` but its `dur`. `EventSequence.from_rows`
reads either shape back, so a list of plain numbers is a sequence a roll opens:

```python
line = seq.to_rows("midinote", "dur", line=True)   # [(None, 0.5), (60.0, 1.0), ([64.0, 67.0], 1.5)]
again = EventSequence.from_rows(line)              # the same notes, at the same beats
```

A score of several voices is not one line, and `separate` takes the sequence
apart: `seq.separate("voice")` answers a new sequence for each voice,
`"staff"` one for each staff and `"channel"` one for each channel. Each part
holds its events with the ids they had, the curves of its channels, the tempo
map and what the whole says of its page, cut to the part — by voice or by
staff a part is a score of its own, whose staff is the top one. Each names the
specification the rule above gives it, so a voice with a glissando separated
from a score in MIDI 2.0 is MPE on its own. A part is read as a line, or put on
a track of its own:

```python
for voice in score.render_events().separate("voice"):
    print(voice.to_rows("midinote", "dur", line=True))
```

### A sequence on its page

A sequence has two presentations, and `view` chooses between them: the roll,
which is its default, and the page it is read into.

```python
take = EventSequence.from_smf(open("take.mid", "rb").read())
page = edit(take, view="score", meter="3/4", division=8)   # the score editor, over the take
roll = edit(take)                                          # and a roll over the same take
```

The sequence is still the structure — `page.structure` is `take` — and the
score on the page (`page.score`) is a reading of it, by the transcription the
keyword arguments say (`Score.from_events` describes them). **Opening changes
nothing**: a take keeps the times and the lengths it was played with, and the
page writes each note on the nearest value it is told to.

**An edit on the page changes in the sequence only what it changed on the
page.** Move one note along its staff and that event's pitch moves, written
as the page spells it; every other event stays as it was played — its time,
its level, its curves, a bend drawn by hand. Write a note in and the ones after
it are later by its length, each as far off the beat as it was. Mark a
dynamic and the levels after it are the mark's, with no note moved in time.
What was written is in the event's notation keys from then on, so the page
reads it back, and what the page says of itself — a meter, a key, a slur —
goes into the sequence's `notation` section where the edit changed it.

**The entry is the sequence's.** The page and every roll over the same
sequence are one history: Ctrl+Z in either window takes back the last edit
wherever it was made, a note dragged in the roll is redrawn on the page, and
`take.history.undo()` walks the same order from the script. What plays is
the sequence itself, as it was performed, and not a render of the page.

A `Score` held as itself is unchanged by any of this: `edit(score)` edits a
document that is saved as MEI, and `score.render_events()` goes one way.
`examples/editors/edit_notes.py` opens a sequence in two rolls and on its
page.

`examples/notation/score_editor.py` is the whole of it: a document opened, operated on, edited,
played, saved and rendered.

## The arrangement: tracks, take lanes, regions

The model a multitrack editor edits is the one the three classic
applications are built over — the audio editor, the multitrack editor and the
score editor, over one document.

The vocabulary is the field's own:

- A **source** is samples. It lives outside the arrangement — the session's
  table says where — and is never overwritten.
- A **region** is one placed thing: a span of the timeline (where it starts, how
  long, its fades, which of the overlapping ones is on top) plus what fills it.
  Six regions over one source are six identities and one source, referenced
  rather than copied. That is the whole of non-destructive editing.
- A **take lane** is one of a track's several contents, an ordered list of regions.
- A **track** holds several take lanes and **plays one**, which is what comping is:
  record six passes into six take lanes, then take from each.
- An **automation** is a curve over one parameter, in the multitrack's time.
- The **multitrack** is the tracks plus what there is one of: the tempo
  map, the meter map, the markers, the loop. They are there and not on a track
  precisely so that no two tracks can disagree about them.

```python
from clausters.multitrack import Content, Fade, Multitrack, Tempo

multitrack = Multitrack()
multitrack.set_tempo(Tempo(at=0.0, tempo=1.6))       # beats per second: 96 a minute
bar = multitrack.tempo_map().secs_at(4.0)            # where the second bar begins

drums = multitrack.tracks.add("drums")               # a Track, with one take lane
hit = drums.active_take_lane.regions.add(bar, 2.5, Content.onto(take))
hit.fade_in = Fade(0.01)
drums.automation.add({"port": "gain"}, [(0.0, 1.0), (8.0, 0.5)], name="gain")
multitrack.markers.add(bar, "verse")

written = multitrack.write()          # the crate's JSON
multitrack = Multitrack.read(written)
```

**What a script reads and writes is objects.** A `Multitrack` is a handle over
the multitrack the shared crate holds — the very one a multitrack editor opened
on it edits — and `multitrack.tracks`, a track's `take_lanes`, a take lane's
`regions`, the curves of a track or a region (`automation`) and
`multitrack.markers` are live collections: iterate them, index them, and `add`
answers the object it made. Each object is a view of what the multitrack holds,
not a copy, so after a hand drags a box in the window the `Region` a script holds
reads where it now is; the same structure read twice is the same object. A
change is made through the object it changes — `hit.position = 4.0`,
`hit.place(other_lane, position=4.0)`, `drums.muted = True`, `hit.remove()` —
and no call takes or answers an id: the ids are the crate's. A tempo entry, a
meter entry, a span, a fade and a region's `content` have no identity of their
own, so they are read as values and written whole through what holds them. A
structure the multitrack no longer holds — removed, or undone away — leaves its
object **detached**: `held` is false and reading it raises, until an undo brings
it back.

**A change is a turn of the multitrack's history, when it has one.**
`multitrack.history` is that history — the undo order every editor open on the
multitrack shares — and asking for it, or opening an editor, is what gives a
multitrack one. From then on each change made through the objects is an entry
the windows redraw and one Ctrl+Z in a window takes back, and `with
multitrack.history("tidy"):` makes everything inside it one entry. A multitrack
nobody asked a history of changes freely and records nothing.

A **region** is the model's word and a **clip** is the picture's: a clip, a track
row, a waveform are what the host draws; a region is what an edit names. And
everything placed is placed in **seconds** — a region, its fades, a curve's
points, a marker, the loop — while what fills a region is measured in its own
source's units, so the two are not the same axis. The tempo and meter maps are
structures the multitrack holds rather than its axis: a ruler draws beats and bars
from them and a snap reads them, and an edit of the tempo moves no region. A
session saved in beats, before this, is converted when it is read.

### Editing a multitrack: the verbs a multitrack admits

The multitrack has an edit vocabulary of its own, and every change made through
its objects is one of its verbs: a region's place is `placeregion`, its length
and window `trimregion`, its fades `faderegion`, a curve's points
`setautomation` and whether it is shown `showautomation`, a marker
`setmarker`. The same vocabulary is reached as data
through the door every other structure is — `domain_edit`, with `MULTITRACK` as
the vocabulary: hand over a multitrack as the crate's JSON and an edit, which
names what it moves by the ids the JSON holds, and take back the multitrack as
it now stands and the edit that puts it back.

```python
from clausters.document import MULTITRACK, domain_edit

edited = domain_edit(
    MULTITRACK, written,
    {"intent": "placeregion", "region": 3, "track": 1, "take_lane": 2,
     "position": 16.0, "layer": 0},
)
edited["applied"]                       # True
edited["state"]                         # the multitrack, as JSON, with the region moved
edited["current"]                       # the edit that puts it back
```

A gesture that comes to several edits — a block of regions dragged, a move
and a split — goes through `domain_edit_all` instead, which applies them **as
one: all of them or none**. One edit refused refuses the gesture, and the
multitrack comes back as it was, with the reason; otherwise each edit that
landed comes back beside the edit that puts it back, to undo in the reverse
order. Calling `domain_edit` once per edit is how a gesture comes to be left
half applied.

```python
from clausters.document import domain_edit_all

gesture = domain_edit_all(MULTITRACK, written, [move, split])
gesture["applied"]                      # False if either was refused
gesture.get("reason")                   # and then why
gesture["pairs"]                        # [{"forward": ..., "backward": ...}, ...]
```

Sixteen verbs, in three groups. What a **track** is: `settracks` (the tracks
now, whole — adding, removing and reordering are one verb, because all three
say the same thing) and `setactivetakelane`, which is comping's one verb. What a
**region** is: `settakelane` (a take lane's regions, whole), `placeregion`, `trimregion`,
`splitregion`, `joinregions` and `faderegion`. What the **multitrack** holds:
`setautomation`, `showautomation` (one curve shown or hidden, which a
reopened multitrack keeps), `setmarker`, `removemarker`, `setrange` (the loop
or the punch span), `settempomap`, `setmetermap` and `setdefaults` (what a
region is when it does not say, and whether overlaps crossfade).

Three things about them are worth knowing before you write against them.

**Moving a region to another track is one edit.** Where a region is means track,
take lane *and* second, and an intent is absolute — so `placeregion` states all three
together. One entry in a history, one undo, and no moment in between where the
region is on no take lane at all.

**A region's fades are its own, with a default.** Every edge of a region —
a box of samples or a box of notes alike — fades: by its own `fade_in`/`fade_out` where
it states one, and by the multitrack's default where it does not, which is
10 ms of equal power until `multitrack.default_fade` says otherwise (`None` for none at all).

**A crossfade is two fades over an overlap**, not a third object. Two regions
on one take lane that overlap are crossfaded by the edit that made the overlap: a box a hand moves,
trims or pastes over another writes the crossfade into the two regions' own fades
— the earlier one's fade out and the later one's fade in cover the overlap —
in the same undo step as the move, so the more they overlap the longer the
crossfade, and there is no crossfade to lose track of or keep in step with the
two fades. Only an edge on the default, or one the crossfade wrote, is written:
an edge the region states — drawn by a hand, set by a script — is kept, shorter
or longer than the overlap. A crossfade follows its overlap when a later edit
moves it, and goes when the overlap does. A script states the fades it wants
and gets them. That is a facility of editing and it can be turned off —
`multitrack.crossfade = False`, a `setdefaults` edit like any other — after which
an edit writes no crossfade and an overlap it makes is the two summed; turning it
off or on changes no region, only the edits that follow.

**A split and a join ask you for the content.** They are the two edits that
change how many regions there are, and the two the crate will not work out on
its own: the cut is on the multitrack's axis and a window into a source is on
the content's — a recording's seconds read at a playrate, a node's own beats —
and this document converts between the two *never*. So `splitregion` takes
`left_content` and `right_content` and `joinregions` takes `content`, from you,
who knows how the content is read. Omit them and both halves go on reading what
the region read.
Both also invert as `settakelane`, the take lane's previous contents: nothing smaller
describes putting back a region that was made out of two.

Everything else the vocabulary does it does the way the tree's does — absolute,
idempotent, refused rather than merged when it was made against a multitrack that has
moved. The multitrack carries **its own version** for exactly that: an editor of the
multitrack is not editing the tree, so one counter for both would make every edit to
either look like a change to both.

## Editing a multitrack: `edit(session)`, and what it plays

A `Multitrack` is one of the fundamental structures, so it opens with the same
verb the others do — and what opens is the **multitrack**, the GUI host's
application: one widget drawing its own ruler, its own track headers, its own
automation rows and its own boxes, in a window of the host's, playing on an
engine of its own.

```python
saved = Session.open(path)              # or a Multitrack, as an untitled session
editor = edit(saved, title="multitrack")
```

The document names a **source id**, never a path; the session's table says
where each source is, and the application reads the takes from there.

**The multitrack sounds.** Each track's regions are read from their files into
the track's readers, and an edit moves where they are read from without making a
node, so a box moved while it plays is heard where it was dropped with nothing
that is sounding cut. The window carries the transport row that goes with it
(rewind, play/pause, stop, and where the multitrack is), and
`editor.transport.play()`, `pause()` and `stop()` are the same verbs from a
script. **The space bar is play/stop**: a stop goes back to the position
cursor, so the play cursor lands where the mark is. **Alt and a drag** anywhere
on the window marks a **time range**, as a drag does in the audio editor: the
space bar plays it from its start to its end, and the loop switch (**L**) loops
it, or the whole multitrack with no range. From a script a pass over a range is
`editor.transport.play(start, end, looping=True)`. **Where a pass ends** is
switched by **Shift+L**: rolling on past the contents, or stopping where the
last region ends — the Transport menu's *Stop at end* and the tool beside Loop
are the same verb, and `editor.stop_at_end()` the script's. A region placed past
an end marker takes the marker with it. An end is where a pass stops and nothing
else: it bounds no view, and a window scrolls and zooms out past it. A session
keeps it as `Session.end`, so a multitrack opened again stops where it stopped
before.

Two cursors, and only one of them is placed: a click on the ruler — or on the
slack between boxes — puts the **position cursor** down, which is where the next
play starts, and a stopped transport is cued there. The playhead is never placed,
so stop goes back to the mark rather than to the top -- and **rewind** is what
puts the mark itself back at the top, which is a statement about the cursor and
not about the transport.

**A curve is heard.** A track's level, its mute and its solo reach what plays
it, and so do the automation drawn on a track and the envelope drawn inside a
box: each names the `gain` port of what it is on -- the same port the header's
knob writes -- so a point dragged while the multitrack plays is heard where it is
drawn.

**And a track shows what it produces.** The strip in each header is one column
per channel over that track's own output, after its clips, its curves and its
fader. It is read in decibels, which is what makes it legible: the column is
green up to the alignment level (-18 dBFS), ambers through the headroom above
it and is red in the last six decibels before full scale, with the peak it
reached held beside it.

`examples/editors/edit_multitrack.py` is the whole of it, by ear and by eye.

## The presentation: what a window shows, beside what the multitrack is

Where a window is looking, how far it is zoomed, what the hand is holding, how
tall each track is drawn — none of that is what the multitrack *is*, and all of it is
state a person loses on a reopen unless something writes it down.

So it is a **`View`**, and it sits **beside** the model rather than inside it.
The shape is Live's and it is deliberate: `Song.View`, `Track.View` and
`Application.View` are objects parallel to their model objects rather than
children, presentation on one side and functional data on the other, both
readable and writable from a script. A `TrackView` is therefore looked up by the
track's id, and an `Multitrack` round-trips the same whether or not a view of it
exists.

```python
from clausters.multitrack import Session, Span, View

window = View(name="arranger", visible=Span(0.0, 48.0), quant=4.0)
window.track_view(10).height = 96.0
window.track_view(10).take_lanes_shown = True      # comping open
window.selected = [20, 32]

session = Session(arrangement=multitrack, views=[window])
```

**There is more than one of them.** A multitrack drawn in two windows has two views
and they disagree on purpose — the arranger snapping to a bar, the editor below
it to a sixteenth — which is why a session carries a list rather than a view.

**A view entry for something the multitrack no longer holds is dropped.** `prune`
does it, and the rule is the one this project already fixed a class of defects
by adopting: state goes when the thing goes. Keeping it is worse than losing it,
because a height kept for a track that is not the same track is a defect that
looks like a feature.

```python
window.prune(multitrack)      # True when something went
```

**What a view never reaches.** Not the document, and not the history: a view is
not edited through an intent, an undo never puts a scroll back, and nothing here
is consulted when an edit is applied. A session file may carry one because
reopening a multitrack into the window it was left in is what every program in the
field does — and a reader that ignores the field opens exactly the same music.

## The document, and who edits it

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the web client, and a GUI host running standalone with
no language attached at all. That is not an implementation detail you can ignore
once you edit from more than one place, so this section says what crosses.

**`clausters.form` has no door to it, and that is deliberate.** It had one until
2026-09-06 — a bridge that converted its elements to the crate's JSON — and it
was removed with the turn that made the arrangement a model of its own. What a
multitrack is written with now is `clausters.multitrack`, above; what the crate's
own document holds is a **leaf as an id, a kind and a configuration it never
interprets**, and a generator travels as a *reference* the way a project file
references a plugin rather than serializing it. A generator *is* code, in the
language that wrote it, so no format owns one; what the document guarantees is
that it does not lose it.

### An edit is applied in one place

`clausters.document` is the door to it — the same surface the web client reaches
through its own `document` module, name for name. The crate is the only thing
that applies an edit. A client does not apply and
then report — it hands over the document and the **intent** and receives the new
document plus what happened:

```python
from clausters.document import apply_intent

result = apply_intent(
    doc,
    {"intent": "place", "node": 3, "offset": 4.3},
    against={"version": doc["version"]},   # the state you were looking at
    quant=1.0,                             # the musical grid, in beats
)
result["outcome"]["effective"]   # {"intent": "place", "node": 3, "offset": 4.0}
result["outcome"]["reason"]      # "snapped to the grid"
```

Three properties are worth knowing because they change how you write against it.

An intent is **absolute**: it states the value the edit *results in*, never an
increment. So applying one twice leaves the same document, and a view that drew
an edit optimistically can leave its picture standing over whatever comes back
instead of recomputing anything.

It states the **whole** value, so **absence is a value**. A `place` describes a
placement entirely: one carrying no `dur` is a placement with *no length*, and
the element's own is what plays. That is not a shorthand for "leave the length as
it is", and where it matters is an inverse — the undo of the first resize of a
clip has no `dur` to carry, because before that resize there was none. The same
holds a level down: a member whose node carries no configuration is a leaf
configured as it was made, which is what an undone trim hands back.

There is **no success flag to branch on**. `effective` is the edit describing the
document as it now stands, so *applied*, *applied transformed* and *refused* are
one shape — a refusal is simply the previous value handed back. `applied` says
whether anything moved and `stale` says whether the refusal was someone else
having changed the document underneath you, which is a different thing to tell a
person than "not here".

A **structural** edit redraws itself. A split, a join or a cut changes which
clips exist, and a widget that was not there cannot travel as a property — so the
editor redefines the window for those, and for an undo of one. A placement, a
length or a curve does not: it is a value the host already has a widget for, and
it travels with the acknowledgement.

### Undo: the history belongs with the document

An editor's undo is not the editor's. The history lives in the same crate as
the document, beside the data it inverts, and `undo` / `redo` step through it:

```python
# after an edit in the window
editor.can_undo                  # True
editor.undo_label                # "move the point"
editor.undo()                    # it springs back, and the window is told
editor.redo()                    # and forward again
```

Wire them to two buttons the way the transport is wired, by name:

```python
win["undo"].on_event(lambda v: editor.undo() if v == 1 else None)
```

The keyboard needs no wiring at all: **Ctrl+Z** and **Ctrl+Shift+Z** over the
window reach the same history, because the host sends them as an `"undo"`
addressed to the *window* rather than to a widget — undo is aimed at no place
under the cursor — and the editor answers it like any other event.

**Why the history is not kept here** is the whole reason it is worth explaining.
A log an editor keeps sees only the gestures *that editor* made — so a script
that edits the data, a second view on the same structure, or a re-render leaves
it describing something that has moved on, and undoing then writes a state
nobody was ever in. One history per structure, wherever the edits come from, is
the only version of this that stays true.

So the history belongs to the **data**, and two windows over one structure find
the same one:

```python
first = edit(curve, sample_rate=sr)
second = edit(curve, sample_rate=sr)      # a second window, same structure
                                          # drag a point in either window
second.can_undo                           # True: it is showing the data that moved
second.undo()                             # and the point springs back in both
```

**Every message reaches every editor**, and that is what the host's event loop
does with them: each open editor is handed the whole stream and answers for the
widgets it drew, so another window's events fall through untouched. An editor is
driven by `apply` rather than by `pump` — `pump` dispatches to the widget handles
a script registered — and the loop calls the first before the second, in one
order.

An edit in one window **reaches** the others, which nothing else would do: an
acknowledgement goes to the window whose gesture it answered. It arrives as
props — the placement, the length, the notes — and only a structural edit (a
split, a cut, an undo of one) redraws them whole, for the same reason a redefine
is not what answers a drag.

That holds for a window over a *part* of the multitrack too — a dedicated roll of one
track edits through the multitrack's history rather than opening a second one
over the same notes. What each window keeps for itself is what a window can see:
its selection, its zoom, which layer the hand is on. None of that is ever an
entry in a history, which is the same line drawn twice.

Two consequences follow, and both are the point rather than a limitation. The
**grid is applied by the crate**, not by the editor: a drag states where the
hand put it, and what comes back is where it landed — so a redo replays the
*snapped* value and cannot snap a second time. And an **inverse is an ordinary
edit**, so undoing needs no second path: it is the same intent machinery running
backwards, and the window adopts the result exactly as it adopts a snap.

### The window's verbs, from a script

Every command a window has — a menu entry, a key, a tool — is a **verb**, and
the verbs are an API: the script that opened the window asks for them the way
the hand does. `editor.verb(name)` asks the host to perform any verb of the
window on what its focus or its main view holds, through the same dispatch a
key and a tool end in, and each verb has a named member that is one line over
it:

```python
editor.select_all()          # Ctrl+A
editor.copy()                # Ctrl+C
editor.to_end()              # End
editor.paste()               # Ctrl+V
editor.verb("view_all")      # R, through the door every member is over
```

What a verb edits comes back as the edit a hand made — recorded, drawn,
undone with Ctrl+Z — and it arrives through the window's event loop, so it is
in the structure a moment after the call returns. A verb needs a window: with
none open, `verb` answers `False`. `docs/verbs.md` in the repository lists
every verb with the member that asks for it.

**A window's picture** is asked for the same way: `editor.capture(path)`
writes what the window shows as a PNG and answers the path once the file is
whole. It is drawn with the render the screen is drawn with, so it is the
screen's picture, and it waits for the turns before it, so what a verb or a
gesture edited is in it:

```python
editor.select_all()
editor.quantize()
editor.capture("out/tidy.png")      # the window, as a PNG
```

The path is on the host's disk, relative to its working directory unless
absolute. With no window open the call raises `RuntimeError`.

**A hand** is played to the window the same way: `editor.drag(start, end)`
presses at `start`, moves along the line to `end` and releases there, and
`editor.click(x, y)` presses and releases at one point. The host hands each
event to the window's gesture machine as the mouse's own, so a scripted drag
is the drag: the grid snaps it and a constraint holds it, and what it edits
comes back as a hand's edit does. The points are in the window's device
pixels, which are a capture's, so what a script finds in the picture is where
it points:

```python
editor.snap_to_grid = False
editor.drag((450, 416), (700, 416))     # a note, a little later
editor.capture("out/dragged.png")
```

**What a window costs** is asked for the same way: `editor.activity()`
answers what the host did for it since it opened — frame ticks, frames drawn
whole or only their live layers, the queries those ticks sent the server, the
bytes downloads brought — and `demand`, the reasons it is ticking now. A
window with nothing moving in it has an empty `demand`: an audio editor that
is stopped draws nothing and asks the server for nothing until something
changes, and a window that cannot be seen is not ticked at all. With them
come what its drawn frames cost by phase: placing the tree, drawing it into
meshes, writing them to the card, recording the pass and presenting it.
`editor.profile(seconds)` takes two readings that far apart and answers what
moved between them, and `format_window_profile` reads it out:

```python
from clausters.gui import format_window_profile

profile = editor.profile(2.0)
print(profile["demand"], profile["live"])   # [] 0 while stopped
print(format_window_profile(profile))
```

`host.activity(window)` and `host.profile(window, seconds)` ask it of any
window.

**What a quantize, a drag and a paste land on is a grid in beats**, a
sixteenth until you choose another: `editor.grid = 1.0` for whole beats, `0`
for none. It is read through the structure's tempo map, so a line is a beat
wherever the tempo puts it, and what a quantize leaves is written into the
document on its beat exactly — a note quantized to the third beat is at
`3.0`, not at a second that rounds near it. The grid itself is the window's,
like its zoom: setting it is no edit.

**Snap to grid is a switch beside it**, on by default: with it off a drag, a
paste and a split land where the hand puts them, and a quantize still moves
onto the grid. G in the window, the View menu and the toolbar's magnet flip
it, the magnet drawn held while it is on, and so does `editor.snap_to_grid = False`.

### What the hand marked, and what it swept

Two things on a multitrack window are marked by hand and are not edits —
nothing in the multitrack changes — and each is read in the type it is of.
**What is held** is regions: a click, Alt+click or a marquee over the boxes,
and `editor.selected` answers them as the `Region` objects of this multitrack.
**What is swept** is a time range, an Alt+drag across the tracks, and it is the
transport's: `editor.transport.span`, in seconds, the one the space bar plays
and `L` loops.

```python
editor.selected                    # [Region(...), Region(...)]
editor.select(list(track.take_lanes[0].regions))
editor.unselect()                  # hold nothing
editor.transport.loop(1.0, 3.0)    # the band drawn, the loop on
editor.transport.span              # (1.0, 3.0)
```

Both are the picture's state: they enter no history, and a script setting one
is drawn the way a hand setting it is. The notes editor reads the same two
words — its `selected` is events, its span the transport's — and the audio
editor's selection is samples.

The other scrap of screen state a driver can ask for is **which layer of a clip
the hand is on** — its placement, its notes, its curve:

```python
editor.edit_layer(element, member)   # "roll", say, or None
```

It is asked for the way every other route here is: by the **placement**. A
widget id is the picture's name for a widget and is minted afresh every time the
window is redrawn, so nothing that has to outlive a redraw is keyed by one — the
same rule the history follows, one level down: identity belongs to the data,
never to the view.

### Cut and paste, and what an editor of placements may do

The host's clipboard verbs reach the editor as two more events, and it answers
them the way it answers everything else — by deciding nothing an intent could
decide:

```python
# Ctrl+X over a selection covering a clip
editor.can_undo              # True: a cut is an edit, so it inverts
```

A cut whose selection **covers a clip** removes that placement, through the
document, undoably. A cut running **across** one implies a new length for the
samples under it, and that one is refused with the reason travelling back, so
the window can say why rather than appearing to ignore the key.

A paste places what the clipboard holds, and the three verbs are **one
mechanism**: a block of notes copied out of a roll is written onto the roll the
paste addresses as an ordinary edit of its notes — the same call a drag on a
note goes through — so it is one entry on the pile and one undo takes the whole
block back. Where the clipboard holds *samples* the answer is a refusal, because
audio with neither a source nor a source's owner is not something an editor of
placements may invent: writing samples is the job of whoever owns them, against
a working copy, which is a different thing from placing elements in time.

The position a paste names is on the **timeline's** axis, and a roll's notes are
in its clip's own time, so a clip placed at beat 2 holds its own note 0 there —
the editor converts, and the block keeps the spread it was copied with.

### Saving: the document plus where its sources are

A document says what plays when and deliberately not where a source lives — in a
running system a source is a server buffer, a mapped file or a rendered result,
and the tree has no business knowing which. A **session** is the document plus
that missing half:

```python
from clausters.multitrack import Session, Source

session = Session(arrangement=multitrack, provenance={"script": "song.py"})
session.sources[7] = Source.file("takes/vocal.wav", lifetime="external")

session.save("song.json")
session = Session.open("song.json")
```

`save` writes the crate's JSON and `open` reads it back; `write` and `read` are
the same two steps without the file, for a caller that keeps it elsewhere.

A source's **lifetime** is what makes saving honest: `external` is the user's own
file, which is never written; `session` is persisted beside the document;
`temporary` is a destructive edit's working copy. Saving in the middle of such an
edit promotes the working copy and **leaves the edit open** — a save is not an
edit, and refusing to save until you decide would block the safest habit in the
program.

`provenance` is a reference to whatever produced something, carried and never
interpreted. It is what makes re-generating possible without the format knowing
how, which is the same rule the opaque generator follows one level down.

A buffer read from a file is written as that file; one allocated in this run is
written **volatile** (`Source.volatile()`) — it existed only while the process
did, and a session that promised otherwise would reopen with silence where it
promised samples. A path inside the session's own folder is written relative, so
the pair of files moves together; one outside it stays absolute, because a
session never claims to own your file.

**A sequence of events is a source too**, held in the file rather than beside
it: `Source.events(notes)` puts an `EventSequence` in the table, and a region
whose window names that source is a window onto its beats, in seconds through
its tempo map. The table holds the handle itself, so a save writes what an
editor did to it. `session.sequences()` hands the handles back. Such a box draws the notes it reads and plays them from the transport's event
lane, into its track: the track's fader, its curves, its mute and its meter are
over its notes as they are over a take (below, "What a multitrack is as
nodes"). And **a double click on it opens its roll** — a notes editor over the
same sequence, in the multitrack's undo order, which edits what the box draws:
both hold one sequence, and nothing is copied.

**A box of notes has a second presentation, its page.** `notes_view="score"`
— an option of the editor, its `notes_view` attribute, and *Notes as scores*
in the View menu — draws every box of notes as notation: the box's sequence
read into a score (`Score.from_events`, with its defaults) and drawn **on the
box's own axis**, every note at its time, in line with what is on the other
tracks. It is proportional notation: a notehead with its stem, its accidental
and its dot is drawn as engraved and moved whole to the note's time, and what
joins notes — a beam, a slur, the staff's lines — stretches between them.
What has no time takes no room on the axis: a meter is written over the
staff, and the first clef and key signature stand before the box's first note
and stay in view when the box's start scrolls off, as a roll's keyboard does.
The staff is fitted to the track's height, and a track too low for one to be
read draws its boxes as rolls. It is the window's view and no edit: the
sequence is what it was, and a double click on a box drawn this way opens the
score editor over the sequence (`edit(sequence, view="score")`, above).

Three questions a save asks the table, and each has an answer rather than an
exception: `session.volatile()` is what is not written down anywhere,
`session.open_edits()` is what is still undecided, and `session.dangling()` is
what the multitrack names and the table does not hold — **every** take lane walked, not
only the ones that play, because an alternate take names its source whether or
not anyone has chosen it yet.

### Reopening: structures, not a description

`Session.read` gives the multitrack and its table back, and by itself that is half a
verb: every take is a bare source number. The other half — what a source *is*
while it plays — is the multitrack application's, which resolves the table as
it opens the session:

```python
from clausters.multitrack import Session

session = Session.open(path)
editor = edit(session)
```

Each take is read from its file **once per source**, a relative path against
the folder the session was opened from: two clips over one take are two windows
onto one take, never two copies that drift apart on the first edit. A **join**
is read from the takes it is made of, so it plays the spans the file states. What
is read and how is the shared crate's, so the web client and `clausters-gui
--session` open the same session the same way.

A *volatile* source opens unresolved rather than as a lie -- its box drawn,
placed and silent -- and the rest of the multitrack opens: half a session is worth
opening. And a generator whose reference `defs` does not have keeps what it last
**rendered** as its floor, which is the same thing a host with no language
attached shows.

### Mixing is the multitrack's

A **track** carries its own mix: `muted`, `soloed` and `level`, the fader as
a linear gain. A muted track is silent, and so is every track that is not
soloed while any track is: the rule is the multitrack's, applied where it is
played, and the document only marks the track.

```python
bass, lead = multitrack.tracks
bass.muted = True
lead.level = 0.5
```

They are fields of the track and are written with it, so a multitrack reopens
mixed the way it was left, and the editor's track header is drawing the
multitrack rather than remembering something of its own — pressing mute there goes through the log and
undoes like any other edit. What is *drawn* is read unmixed: a muted track keeps
its clips, its notes and its length, because a picture that emptied when the
toggle was pressed would report silence as absence.

A track's **height** is the other kind of thing and is in no document. It says
nothing about what the multitrack is; resizing a track (Ctrl+wheel) changes the view
and no file.

### What a multitrack is as nodes: the channel strip, three times

A multitrack is a picture and a sound, and this is the second one. What plays it is
not a driver a script writes: it is three GraphDefs over **one** shape, the
channel strip every fixed-channel mixer has had for fifty years.

```text
in -> [pre-fader inserts] -> fader (+ mute) -> pan/width -> [post inserts] -> out
```

A **track** is that strip over what its regions read, and the **master** is
that strip over its tracks. A region's own gain, its fades and its envelope are
read with its samples -- the track's reader sums the regions under the play
position, each at its gain -- and a region's gain and a track's gain are both
real and they are different stages: the first corrects the take, the second
mixes it, which is why an envelope on a region is not another name for the
track's fader.

**A box of notes is a strip of its own.** Its notes are voices the transport's
event lane makes and frees as it plays, and each is told the bus of its box as
its `out`; the box is the same strip over that bus, into its track. So the
track's fader, its curves, its mute and its meter are
after a note as they are after a take, and the box has its own. What a note is
before that is the note's: its def, its velocity, the curves inside it. What
this asks of a def is the convention an event already has — an `out` control
that names the bus it writes, and the one after it for a second channel. The
built-in `default` has one; a def without it sounds where it was written to,
past the track. A MIDI message or a command in a box goes where it names, and
no strip reaches it — a muted track leaves those out instead.

The multitracks of it are named once, in `clausters_core::mixer`, and both clients
bind the same names: `gain`, `pan`, `width`, `mute`, and a box's own `at`,
`span` and `start`. **The knob in the header, the automation curve and a
`/node_set` all write the same port of the same instance** -- that is what makes
an automation whose target says `gain` drive the gain the header shows, rather
than a second thing spelled the same.

Two words are worth keeping apart, because confusing them is the classic mixer
bug and `pan` is one name for both:

- Over a **mono** source, `pan` is a pan: an equal-power law that puts about
  -3 dB on each side at the centre. The same signal sent to both sides instead
  would be 3 dB too loud in the middle.
- Over a **stereo** source, `pan` is a **balance**: it attenuates one side and
  leaves the centre untouched. A pan law here would take 3 dB off every strip,
  and there are three of them in a row.
- Past **two channels** there is no image to move: a track as wide as its
  widest region -- up to eight channels -- has a strip of gains, and `pan`
  and `width` move nothing on it. Its channels reach the master through a
  fixed matrix: each channel the master has reads the track's channel of the
  same number, and the track's channels past the master's are routed onto the
  master's first ones again, so a six-channel track on a stereo multitrack is
  heard whole, folded onto two. Its meter is a column per channel.

Which one applies follows from the source's width -- `Track.channels` and
`Multitrack.channels`, both fields of the document, because the width decides
the mix and reopening a multitrack has to give back the mix it was left with.
A track's fader is `Track.level`, a field for the same reason: what a multitrack
sounds like is the multitrack's, not a key one client reads out of a table it was
only meant to carry.

A strip writes an output bus of its own and a **send** carries it onward at a
gain -- one node between a track and the master. That is what lets a meter mean
something: every track writes into the master's mix, so a meter there would read
the sum and call it the track. A meter is a **slot** on a strip, so a multitrack
nobody is looking at holds none, and each one writes one control bus per channel
-- one number a block, with the fall and the peak hold applied by the engine, so
two windows cannot draw two different falls off one signal.

And the transport plays the multitrack rather than a client stepping it: each
track's reader reads the position the engine publishes, so moving a box hands
the track its regions again and a locate is no message at all.

### Where a recording lands

A `Buffer` holds a take and a `RecordingStream` follows one as it is written,
and neither puts one in a multitrack. `take` does:

```python
from clausters.form import take

song.add(take(recorded, instrument="player"), offset=8.0)
```

It is a `Vector` whose length is the samples' own — frames over the rate they
were recorded at — which is the one line every script used to write by hand.
Without an `instrument` it is structure: it draws and it extends the multitrack, and
it emits no event, which is the `Vector` rule rather than a special case.

### Name what the file cannot carry

A document holds a **reference** to an algorithm and never the algorithm — a
generator is code, in the language of whoever wrote it. So reopening hands each
reference to whoever can resolve it and takes back whatever that caller has,
which means the reference must be something you can produce on the way back
in. A def and an
automation carry a name of their own and need nothing; a pattern does not, so
name the **element**:

```python
bass = Sequence(pbind(midinote=pseq([48, 55], 2), dur=1.0), name="bassline")
```

A name is a label, not an identity: nothing addresses an element by it, and two
elements may share one — which is what naming *the same algorithm used twice*
looks like. An **unnamed** leaf is written with no reference at all and comes
back **frozen**: drawn, placed, silent, contributing its extent and emitting
nothing. That is not the file being lossy; it is what a multitrack means
somewhere its language is not running, and it is what a `standalone` host with
no interpreter shows for every generator in the multitrack.

The same name is what a view labels a track with, so naming a track
is worth doing before it is worth needing.
