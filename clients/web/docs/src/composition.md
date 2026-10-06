# The document: what a multitrack is, and who edits it

A `Timeline` places items at beats and plays them. That is enough to
sequence, but not enough to *compose*: a multitrack places recorded and
generated contents in time, with tracks, takes and curves that are **authored,
durable and undoable** — state a picture cannot hold, because a picture is
drawn and drawing frees.

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

The `form` namespace is a **relegated** client-side layer for placing elements
in time, kept frozen and taking no new work; it has no view and nothing is
designed around it. See [`form`](form.md).

## What an editor is

`gui.Editor` edits **one structure** — a buffer's samples, a break-point curve, a
timeline of events — and it knows nothing about any arrangement. That is the whole
of it, and it is deliberately the plain case: editing a curve is what an editor is
for.

An editor orchestrates rather than performs, and it is four collaborators
(`gui/editing/`):

| | what it is | what it deliberately is not |
|---|---|---|
| `View` | the picture of one structure, and the registry from widget id to what it shows | not the vocabulary: one structure is drawn several ways |
| `Domain` | what a gesture needs read with it, and the applied payload written onto the client object | not **what a gesture means** and not **how an edit inverts** — both are the shared crate's, so neither is written once per language — and it does not draw |
| `Echo` | the acknowledgement: the stamp, the version, the corrections, the reason | not anything about what was edited |
| `Editing` | the editing context: the history, and the views to tell | **not the editor's** — it is asked for, never built, which is what makes two windows walk one undo order |

The rule that fixes all four: an editor owns **neither the data nor the
history**. `View` here is not `guidef`'s `View`, which is a tree you can open.

## `edit(x)`: one verb over the four structures

`gui.edit` opens whichever editor the structure asks for, and it dispatches on
**what the structure holds**, never on its class — a curve editor asks for break
points it can read and write back, and everything that answers opens, which is
why the curve row below names three unrelated types:

| `edit(x)` where x is | opens | over | its vocabulary |
|---|---|---|---|
| a `Buffer` | `AudioEditor` | a `waveform` | `parts` |
| a curve — a `Bpf`, an `Env`, a `multitrack.Automation` | `PointsEditor` | a `bpf` | `points` |
| an `EventSequence`, or a `Timeline` rendered into one | `NotesEditor` | a `pianoroll` | `events` |
| a `Multitrack` | `MultitrackEditor` | a `multitrack` | `clips`/`tracks` |
| a `Score` (a symbolic score) | `ScoreEditor` | a `score` page | `score` |

**An `EventSequence` opens in the notes editor**, `editing.NotesEditor`, and is edited **in place**: it holds the very sequence the page's handle names, so there is nothing to write back. **It plays what it edits**, on its sequence's own transport, so playing it never moves a multitrack nor another roll's sequence: `await editor.play()` (from the position cursor, or a beat given), `pause()`, `resume()`, `stop()`, and the space bar over the window, which is play/stop -- a stop goes back to the position cursor. **Where a pass ends** is `editor.end`: `null` by default, the transport rolling on past the last note as a multitrack's does; `"contents"`, where the last note ends; or a beat, an end marker. A note's release rings out past a stop, which releases the notes rather than freezing them. An edit while it plays is heard at once -- the transport's event lane takes the sequence again and plays on from where the position is -- and a note already sounding ends as it would have. The roll's play cursor is that transport's position, stopped or rolling, and a click on its ruler places the **position cursor**: a stopped transport is cued there, and the next play starts from it. **The roll's vertical axis can be a frequency**: `yAxis: "hz"`, on `NotesEditor` or through `edit`, draws the same notes on a log scale ruled in hertz, where a note an octave up is as high as on the keys and a drag moves it continuously, writing its `freq` -- the MIDI note it was written with follows it. Two editors over one sequence, one in each axis, are two windows onto one history. **A page edits the same sequence while the roll is open**, through its objects: the change is a turn of that history, so the roll redraws, the lane plays it, and Ctrl+Z in the window takes it back — `seq.history.entry("humanize", () => { ... })` makes a whole loop one entry. **What the hand marked is the page's to read**: `await roll.selected()` is the events marked on the roll (a click, Shift+click or a marquee) as the `SeqEvent` objects they are, `await roll.select(events)` marks them from the page and `await roll.unselect()` clears — the picture's state, entering no history. The other selection, the time range an Alt+drag sweeps, is the transport's `span`, and `roll.transport` is that transport (see [The transport](transport.md)).

**A `Buffer` opens in the audio editor**, `editing.AudioEditor`.
It writes nothing it was handed while it edits. Its window draws a **join** the editor owns, and
every edit -- a cut, a paste, a mix (Ctrl+Shift+V), a pencil stroke -- leaves a
new list of spans over the take and over the takes the edits made: a stroke is a
new take the size of the stroke, spliced over the frames it was drawn on. An undo
is the list before, stitched again, so it costs the list and not the samples.
The takes a history can still reach are kept, and freed when it cannot;
`historyBytes` caps what only the history holds, and `residentBytes` how much of that stays in memory -- past it the oldest takes are written to the page's own storage (`scratch`) and read back when an undo reaches them. What it opens is a file or a server buffer, and it edits a private copy of it: `await editor.save()` writes the edited take over what it was opened from -- the file it was read from, or the buffer, rewritten whole at the take's length -- and Ctrl+S in the window does the same. `editor.save({ path })` writes it as another file and `editor.save({ buffer })` into another buffer (`buffer: true` for a new one), which a later save then writes over. `editor.buffer` is the edited
take, to play or read, and `editor.parts` what it is made of. **It plays what it edits** on the audio editors' transport, and `editor.transport` is that `Transport` in the take's seconds: the space bar over the window plays the marked range, or from the position cursor, and `L` loops it -- the same `await transport.play()`, `span` and `loop(start, end)` from a page, each side reading what the other set. What the hand marked is samples, and `await editor.selected()` answers them as a `Segment` over `editor.buffer` (its `start` a frame, its `duration` seconds); `await editor.select(segment)` marks one and `await editor.unselect()` marks nothing. In an audio editor the samples marked and the transport's `span` are one range, so setting either sets both.

```ts
const editor = await gui.edit(curve, { sampleRate: 48_000, stage: element });

curve.toPoints();      // the edited curve, out of the object you already held
```

**It opens.** The window is up and listening when that resolves, so the
structure you already hold is the edited one from that moment: read it whenever,
and it says what the hand has left there. `{ open: false }` builds the editor
without a window, for a caller composing one. It is `await`ed where the reference
client's `edit` is not, for the reason `plot` and `View.open` are — resolving the
ambient host may have to boot it.

Nothing is handed back: the object passed in *is* the edited one -- except a buffer, which the audio editor writes back when it is saved. A **multitrack** is
one of them, and what opens is the multitrack editor — the rest of this
chapter is what it edits.

Reading it back **after the hand is done** is `wait`:

```ts
await editor.wait();   // resolves when the window is closed
curve.toPoints();      // the curve, as it was left
```

`editor.closed` is the same question asked without waiting, and `editor.close()`
closes the window from the page — the history is not closed with it, since an
undo order belongs to the data.

**Two calls over one structure give two windows and one stack.** The editing
context belongs to the data, so an undo in either window steps the one order both
of them made. And a window composing several structures passes one context
(`edit(x, { context })`), which is what makes it undo across a curve and a roll
in the order the edits happened.

**How an edit inverts is the shared crate's.** For a curve and a timeline the
state goes in with the payload and comes back as what the structure now is *plus*
what puts it back — one call, because the inverse has to be read before the edit
lands. A span of samples is the exception, and a real one rather than an
omission: the frames are in a server buffer, so the crate holds no state to
invert. What it shares there is the payload's shape and its coalesce key, and the
inverse rides on the wire — a stroke's event carries the run it wrote *and* the
run it replaced. The page's buffer calls are asynchronous, so a stroke's write is
**queued in order** rather than awaited; the Python client writes synchronously,
and that is the only difference between the two.

## The score editor: `edit(score)`

**A `Score` opens in the score editor**, `editing.ScoreEditor`, and is edited **in
place**: the editor is the shared crate's and holds the very score the
page's handle names, so `score.sheet()` and `score.mei()` read every
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
ids, and `editor.select(elements)` selects from the page by the page's
`xml:id`s.

**Writing is a mode, note entry**, and the window opens outside it: N, the
toolbar's pencil or `editor.entry = true` enter it, and Escape, N again or a
play leave it. In it an **edit cursor** stands on a staff, in a voice, at a time
— on what was selected, or where it was last left, or at the first beat — and
what is entered is written there **over what was there**: nothing after it
moves, in the bar or past it, and a value longer than what is left of the bar
goes on into the next one, tied. A letter `a` to `g` writes that pitch, in the
octave nearest the note before it, of the value in hand (`editor.value = [1, 8]`
writes eighths; the digits 1 to 7 pick a value, from a 64th to a whole, `.` the
dot and `0` a rest), and the cursor goes on by it; Shift and a letter add the
pitch to the chord just written. The arrows move the cursor a note at a time,
Ctrl and an arrow a bar, Alt and Up or Down to the next staff, and Ctrl+Alt+1 to
4 put it in that voice — which is how a second voice is written against the
first; Up and Down move the note just written a step, Ctrl and them an octave. A
press on a staff writes at the time it fell at, or, on a note of the cursor's
voice, adds the pitch pressed to its chord. Every one of these keys is a binding
of the host's key table, under `[gui.keys.note_entry]`, and `N` under
`[gui.keys.score]`.

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
with none it is armed for the next note (`nextAccidental`) and let go
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

### The verbs, over what is selected

`move`, `scale`, `articulation`, `dynamic`, `ornament`, `grace`,
`accidental`, `clearMarks`, `tie`, `silence`, `delete`, `voice`, `mark` (a
tremolo, a roll, a breath, let it ring, a fingering, a chord symbol), `lyric`,
`beatRepeat`, `control` (a tempo mark, a direction, a rehearsal mark),
`setClef` and `spanner` (from the first selected note to the last) act on
what is selected, and `operate` applies any model operation. The measure
verbs — `insertMeasures`, `removeMeasures`, `setBarline`, `setBreak`,
`setMeter`, `setKey`, `setEnding`, `navigation` and
`measureRepeat` — act on the measures the selection covers, `setStaff` and
`group` on its staves, and `multirests` on the whole score. Each is one call
into the crate and one entry of the editing context's history, so Ctrl+Z over
the window walks it back.

**A transformation takes the measures the selection covers**, or everything
with nothing selected: `editor.transform("transpose", { semitones: 12 })`, and likewise `"invert"`,
`"retrograde"`, `"stretch"` and `"repeat"`.

### The paper, the layout and the page's text

**The paper is the score's and the layout is the window's.** The score is drawn
on its paper — A4 until somebody chooses — as fixed pages one under another: a
window resize shows more or less of them and never re-flows the music, a drag
on blank paper pans and Ctrl with the wheel zooms. `editor.setPage("Letter", { landscape: true, staff: 800 })` lays it
out on another (a paper by name — `editor.page.papers` lists them — or a `width` and
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
the foot on the left; the page number is written from the second page on.
`editor.setText("title", "Eight bars")` writes one, `editor.setText("note", "* a footnote")` adds a footnote, and
`editor.setText("composer", null, { halign: "left", pages: "all" })` moves one — each an entry of the history. A click on a text
says which field it is, and where it is written, on the status line, and **a
double click types over it where it is drawn**: the whole of it selected, so
the first key replaces it; Enter, or going elsewhere, writes it as one entry
of the history, and Escape leaves it as it was.

### Playing the score

**The score plays on the server, and the page's cursor follows it.** The space
bar over the window, the toolbar's transport (back to the start, play, the loop
switch) and the Play menu play the score on a transport of its own —
`await editor.play()`, `await editor.stop()`, `await editor.playing()`, and `editor.transport` for the
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
own host, opened with `await session.gui()` before the editor, is one; a host that
stands alone still edits the score and draws no cursor.

### The score's file

**A score is saved as itself.** `await score.write(path)` writes it to a file as MEI —
notes, marks, slurs and hairpins, the meter and the barlines, the page setup
and the page's text, not a render of any of it — and `await notation.Score.read(path)` reads one
back as the same page; it also reads MusicXML, ABC and Plaine & Easie,
whichever the engraver finds the file to be. A path names the disk under node and the page's own storage in a tab. The
score remembers its file (`score.path`), which is what the File menu's **Save**
and Ctrl+S write to; a score with none is asked for one, as **Save as...**
always asks, and **Open...** reads another document in place of the score, as
one step of the history. The same two are `editor.save()` and
`editor.load(path)`. **New** puts an empty score in its place the same way —
one staff in the treble clef, common time, C major, four empty bars, and no
file yet — and **Close** closes the window, asking first when the score has
changes its file does not hold (`editor.unsaved`): save, don't save, or cancel.

**A score is exported as what it renders.** The File menu's **Export MIDI...**
and **Export clip...** — `await editor.export(path)`, which takes the format from the
extension or as `"smf"` or `"clip"` — write the rendered sequence as a Standard
MIDI File or a MIDI 2.0 Clip File: the notes, a channel to a voice, the
dynamics as each channel's expression, the pedal on controller 64, at the
score's own tempo. It is the same
writing a sequence does (`toSmf`, `toClip`), so what is
exported is what the roll shows.

### The score into a sequence, one way

`score.renderEvents()` is the verb a timeline and a pattern have: it answers an
`EventSequence`, which is what plays and what a roll edits —
`edit(score.renderEvents())` is the score as notes with their automation — and nothing
made from that sequence travels back to the score.

Every sounding note is an event, a chord one event per note, and each still
says what it is on the page (`seq.NOTATION_KEYS`): the pitch as it is written, its
value, its staff and voice, its marks. A voice renders on a channel of its own.
A staff's dynamics and hairpins are a curve of its channels as well as each
note's attack; on a staff with two voices the same curve is on both channels,
named as one group. A hairpin is straight in amplitude from its first note's
onset to its last one's, and one that ends on no dynamic holds the level it
reached until the next one. A note held under it carries the hairpin as its
own curve over `amp` too, so a crescendo moves inside a held note on a synth,
which listens to no controller. Which of the two an instrument hears is the
reading's — a struck sound hears the attack alone, a sustained one the curve
as well: `interp: { ...notation.interpretation(), dynamics_as: "attack" }` writes no curve and no
note carries the hairpin, `"curve"` leaves the attacks unmarked, and `dynamics_cc` names the controller
(11, expression). What is no note's — the meter, the key, the clefs, the page
and its text, a slur — is kept in the sequence's `notation` section.

`examples/notation/score-editor.html` is the whole of it: a document opened, operated on, edited,
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

```javascript
import { Content, Fade, Multitrack, Tempo } from "clausters";

const multitrack = new Multitrack();
multitrack.setTempo(new Tempo({ at: 0, tempo: 1.6 }));   // beats per second: 96 a minute
const bar = multitrack.tempoMap().secsAt(4);             // where the second bar begins

const drums = multitrack.tracks.add("drums");            // a Track, with one take lane
const hit = drums.activeTakeLane.regions.add(bar, 2.5, Content.onto(take));
hit.fadeIn = new Fade(0.01);
drums.automation.add({ port: "gain" }, { points: [[0, 1], [8, 0.5]], name: "gain" });
multitrack.markers.add(bar, "verse");

const written = multitrack.write();   // the crate's JSON
Multitrack.read(written);
```

**What a page reads and writes is objects.** A `Multitrack` is a handle over
the multitrack the shared crate holds — the very one a multitrack editor opened
on it edits — and `multitrack.tracks`, a track's `takeLanes`, a take lane's
`regions`, the curves of a track or a region (`automation`) and
`multitrack.markers` are live collections: iterate them, index them with
`item(i)` where Python indexes, and `add` answers the object it made. Each
object is a view of what the multitrack holds, not a copy, so after a hand drags
a box in the window the `Region` a page holds reads where it now is; the same
structure read twice is the same object. A change is made through the object it
changes — `hit.position = 4`, `hit.place(otherLane, { position: 4 })`,
`drums.muted = true`, `hit.remove()` — and no call takes or answers an id: the
ids are the crate's. A tempo entry, a meter entry, a span, a fade and a region's
`content` have no identity of their own, so they are read as values and written
whole through what holds them. A structure the multitrack no longer holds —
removed, or undone away — leaves its object **detached**: `held` is false and
reading it throws, until an undo brings it back. A multitrack is a handle over
the core, so the core is loaded (`loadCore`) before one is made.

**A change is a turn of the multitrack's history, when it has one.**
`multitrack.history` is that history — the undo order every editor open on the
multitrack shares — and asking for it, or opening an editor, is what gives a
multitrack one. From then on each change made through the objects is an entry
the windows redraw and one Ctrl+Z in a window takes back, and
`multitrack.history.entry("tidy", () => { ... })` — Python's `with
multitrack.history("tidy"):` — makes everything inside it one entry. A
multitrack nobody asked a history of changes freely and records nothing.

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
`setautomation`, a marker `setmarker`. The same vocabulary is reached as data
through the door every other structure is — `domainEdit`, with `MULTITRACK` as
the vocabulary: hand over a multitrack as the crate's JSON and an edit, which
names what it moves by the ids the JSON holds, and take back the multitrack as
it now stands and the edit that puts it back.

```javascript
import { document as doc } from "clausters";

const edited = doc.domainEdit(doc.MULTITRACK, written, {
    intent: "placeregion", region: 3, track: 1, take_lane: 2,
    position: 16, layer: 0,
});
edited.applied;                     // true
edited.state;                       // the multitrack, as JSON, with the region moved
edited.current;                     // the edit that puts it back
```

Fourteen verbs, in three groups. What a **track** is: `settracks` (the tracks
now, whole — adding, removing and reordering are one verb, because all three
say the same thing) and `setactivetakelane`, which is comping's one verb. What a
**region** is: `settakelane` (a take lane's regions, whole), `placeregion`, `trimregion`,
`splitregion`, `joinregions` and `faderegion`. What the **multitrack** holds:
`setautomation`, `setmarker`, `removemarker`, `setrange` (the loop or the punch
span), `settempomap` and `setmetermap`.

Three things about them are worth knowing before you write against them.

**Moving a region to another track is one edit.** Where a region is means track,
take lane *and* second, and an intent is absolute — so `placeregion` states all three
together. One entry in a history, one undo, and no moment in between where the
region is on no take lane at all.

**A crossfade is two fades over an overlap**, not a third object: `faderegion`
on each of the two regions, which is what the model already holds. There is no
crossfade to lose track of, and nothing to keep in step with the two fades.

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

## Editing a multitrack: `edit(multitrack)`, and what it plays

A `Multitrack` opens with the same verb the others do, and what opens is the
multitrack **editor**: one widget drawing its own ruler, its own track headers,
its own automation rows and its own boxes, over the same picture and the same
reading of a gesture the standalone host uses.

```ts
await session.gui();
const editor = await gui.edit(multitrack, {
    sampleRate: 48_000, server, sources: { 1: take, 2: other },
    title: "multitrack", stage: element,
});
```

`sources` is the one fact about a multitrack that is not in the multitrack: the document
names a **source id**, never a path and never a buffer number, so which buffer
each source was read into travels beside it.

**Given a `server`, the multitrack sounds.** The editor keeps one resident reader per
box in a group the server's transport governs, and puts them where the multitrack says
on every edit whoever made it — this window's gesture, a second window over the
same multitrack, or a step of the history. Moving a box while it plays is one
`/node_set` on a node that is already running, so it is heard where it was
dropped with nothing that is sounding cut. The window carries the transport row
that goes with it (rewind, play/pause, stop, and where the multitrack is), and
`editor.play()`, `pause()`, `stop()` and `rewind()` are the same verbs from a
page. **The space bar is play/stop**: a stop goes back to the position
cursor, so the play cursor lands where the mark is. **Alt and a drag** anywhere on the window marks a **time range**, as a
drag does in the audio editor: the space bar plays it from its start to its end,
and the loop switch (**L**) loops it, or the whole multitrack with no range.
From a page both are the transport's: `editor.transport` is the `Transport` the
multitrack plays on, in seconds, so `await editor.transport.loop(1, 3)` draws the
band and turns `L` on, and a range swept by hand reads back as
`editor.transport.span` (see [The transport](transport.md)). What the hand holds
-- a click, Alt+click or a marquee over the boxes -- is `await editor.selected()`,
the `Region` objects of this multitrack; `await editor.select(regions)` holds
them from the page and `await editor.unselect()` holds nothing. **Where a pass ends** is
the transport's `end`: `null` by default, the transport rolling on past the
contents; `"contents"`, where the last region ends; or a number of seconds, an
end marker -- the same three a notes editor's `end` takes, its marker a beat. A multitrack opened with
no server still edits; it is simply not heard.

Two cursors, and only one of them is placed: a click on the ruler — or on the
slack between boxes — puts the **position cursor** down, which is where the next
play starts, and a stopped transport is cued there. The playhead is never placed,
so stop goes back to the mark rather than to the top -- and **rewind** is what
puts the mark itself back at the top, which is a statement about the cursor and
not about the transport.

**A curve is heard.** A track's level, its mute and its solo reach the readers,
and so do the automation drawn on a track and the envelope drawn inside a box:
each names the `gain` port of the node it is on -- the same port the header's
knob writes -- so a point dragged while the multitrack plays is heard where it is
drawn.

**And a track shows what it produces.** The strip in each header is one column
per channel over that track's own output, after its clips, its curves and its
fader. It is read in decibels, which is what makes it legible: the column is
green up to the alignment level (-18 dBFS), ambers through the headroom above
it and is red in the last six decibels before full scale, with the peak it
reached held beside it.

`examples/editors/edit-multitrack.html` is the whole of it, by ear and by eye.

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

```javascript
import { Session, Span, View } from "clausters";

const window = new View();
window.name = "arranger";
window.visible = new Span(0, 48);
window.quant = 4;
window.trackView(10).height = 96;
window.trackView(10).takeLanesShown = true;      // comping open
window.selected = [20, 32];

const session = new Session();
session.arrangement = multitrack;
session.views = [window];
```

**There is more than one of them.** A multitrack drawn in two windows has two views
and they disagree on purpose — the arranger snapping to a bar, the editor below
it to a sixteenth — which is why a session carries a list rather than a view.

**A view entry for something the multitrack no longer holds is dropped.** `prune`
does it, and the rule is the one this project already fixed a class of defects
by adopting: state goes when the thing goes. Keeping it is worse than losing it,
because a height kept for a track that is not the same track is a defect that
looks like a feature.

```javascript
window.prune(multitrack);     // true when something went
```

**What a view never reaches.** Not the document, and not the history: a view is
not edited through an intent, an undo never puts a scroll back, and nothing here
is consulted when an edit is applied. A session file may carry one because
reopening a multitrack into the window it was left in is what every program in the
field does — and a reader that ignores the field opens exactly the same music.

## The document, and who edits it

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the Python client, and a GUI host running standalone with
no language attached at all.

**`form` has no door to it, and that is deliberate.** It had one until
2026-09-06 — a bridge that converted its elements to the crate's JSON — and it
was removed with the turn that made the arrangement a model of its own. What a
multitrack is written with now is `arrangement`, above; what the crate's own document
holds is a **leaf as an id, a kind and a configuration it never interprets**, and
a generator travels as a *reference* the way a project file references a plugin
rather than serializing it. A generator *is code*, in the language that wrote it,
so no format owns one; what the document guarantees is that it does not lose it.

A leaf whose reference nothing here can resolve is **frozen** — it draws, it
holds its place, and it makes no sound. That is the floor, not a failure: it is
what a multitrack means where the language that wrote it is not running.

Because a document is one format for several languages, the two event keys this
language spells its own way (`addAction`, `hasGate`) are written the way the file
and the wire say them (`add_action`, `has_gate`) and read back the same way. Every
other key is a def's control name, which is one string in every language.

## Saving: the document plus where its sources are

A document says what plays when and deliberately not where a source lives — in a
running system a source is a server buffer, a mapped file or a rendered result,
and the tree has no business knowing which. A **session** is the document plus
that missing half:

```ts
const session = new Session();
session.arrangement = multitrack;
session.provenance = { page: "song.html" };
session.sources.set(7, Source.file("takes/vocal.wav", "external"));

await session.save("song.json");
const reopened = await Session.open("song.json");
```

`save` writes the crate's JSON and `open` reads it back; `write` and `read` are
the same two steps without the file, for a caller that keeps it elsewhere. The
file is on the same filesystem `Buffer.read` names: the disk under node, and the
page's own storage (`opfs`) in a tab.

A source's **lifetime** is what makes saving honest: `external` is the user's own
file, which is never written; `session` is persisted beside the document;
`temporary` is a destructive edit's working copy. A session whose table does not
cover its own multitrack is what `session.dangling()` reports, rather than reopening
with a take that draws nothing and nothing saying why.

`provenance` is a reference to whatever produced something, carried and never
interpreted. It is what makes re-generating possible without the format knowing
how, which is the same rule the opaque generator follows one level down.

Three questions a save asks the table, and each has an answer rather than an
exception: `session.volatile()` is what is not written down anywhere,
`session.openEdits()` is what is still undecided, and `session.dangling()` is
what the multitrack names and the table does not hold — **every** take lane walked, not
only the ones that play, because an alternate take names its source whether or
not anyone has chosen it yet.

A buffer read from a file knows its `path` and is written as that file; one
allocated in this run is written **volatile** — it existed only while the page
did, and a session that promised otherwise would reopen with silence where it
promised samples. A path inside the session's own folder is written relative, so
the pair of files moves together; one outside it stays absolute, because a
session never claims to own the user's file.

**A sequence of events is a source too**, held in the file rather than beside
it: `Source.events(notes)` puts an `EventSequence` in the table, and a region
whose window names that source is a window onto its beats, in seconds through
its tempo map. The table holds the handle itself, so a save writes what an
editor did to it. `session.sequences()` hands the handles back, and with what
`load` answers it is the whole table a multitrack editor is opened with:
`edit(session.multitrack, { sources: new Map([...await session.load(), ...session.sequences()]) })`.
Such a box draws the notes it reads and plays them from the transport's event
lane, and **a double click on it opens its roll** — a notes editor over the
same sequence, in the multitrack's undo order, which edits what the box draws:
both hold one sequence, and nothing is copied.

## Reopening: structures, not a description

`Session.read` gives the multitrack and its table back, and by itself that is half a
verb: every take is a bare source number and nothing has loaded it. `load` is
the other half — what a source *is* in a running system is not the document's to
decide, so it is said by loading the table into a server:

```ts
const session = await Session.open(path);
const buffers = await session.load(server);
```

The answer is a `Buffer` per source, keyed by source id — the same table an
editor takes as its `sources`. A relative path is read against the folder the
session was opened from, on **the server's** filesystem, which is the one that
reads the files. Each take is read
**once per source**: two clips over one take are two windows onto one buffer, and
reading it twice gives them two buffers that drift apart on the first edit. A
**join** is stitched from the takes it is made of, after they have loaded, so it
plays the spans the file states; a take only a join reads is loaded for it. What
is read and in what order is the shared crate's, so the Python client and the GUI
host open the same session the same way.

A *volatile* source is left out with a warning rather than returned as a lie,
and the rest of the multitrack opens: half a session is worth opening. A file that is
not there is the server's refusal of its read, and `load` rejects after freeing
what it had made. And a generator whose reference `defs` does not have keeps what
it last **rendered** as its floor, which is the same thing a host with no
language attached shows.

Loading is asynchronous here and not in the Python client, so the `await` is the
language's and not a different call.

## Mixing is the multitrack's

A **track** carries its own mix: `muted`, `soloed` and `level`, the fader as
a linear gain. A muted track is silent, and so is every track that is not
soloed while any track is: the rule is the multitrack's, applied where it is
played, and the document only marks the track.

```ts
const [bass, lead] = multitrack.tracks;
bass.muted = true;
lead.level = 0.5;
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

## What a multitrack is as nodes: the channel strip, three times

A multitrack is a picture and a sound, and this is the second one. What plays it is
not a driver a script writes: it is three GraphDefs over **one** shape, the
channel strip every fixed-channel mixer has had for fifty years.

```text
in -> [pre-fader inserts] -> fader (+ mute) -> pan/width -> [post inserts] -> out
```

A **clip** is that strip over its readers, a **track** is that strip over its
clips, and the **master** is that strip over its tracks. A clip's gain and a
track's gain are both real and they are different stages -- the first corrects
the take, the second mixes it -- which is why an envelope on a clip is not
another name for the track's fader.

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
-- one number a block, with the fall and the peak hold applied on the server, so
two clients cannot draw two different falls off one signal.

And the transport plays the multitrack rather than a client stepping it: each reader
reads the position the engine publishes, so moving a box is one `set` and a
locate is no message at all.

## A cut that plays as one reader

A box is a window onto a buffer, and a cut assembled from several takes -- or
from one take in another order -- is not that. `Buffer.stitch` installs a
**join**: a buffer whose samples are spans of other buffers, read as one.

```typescript
const cut = await Buffer.stitch(
    [{ source: verse, start: 0, frames: 2 * 48000 },
     { source: chorus, start: 4800, frames: 48000 }],
    { server },
);
```

Without it a reader would have to change which buffer it reads with sample
accuracy, and the buffer a reader reads is an initial-rate control: every seam
would be a new node and a control message in the middle of playback -- which is
exactly what a multitrack that plays itself from the transport must not need. The
fades on a part are the few milliseconds an editor puts on a cut; a source is
held for as long as the join exists, so freeing a take something was cut from
does not silence it.

A join **owns no samples**, so everything that writes into it refuses: writing
would mean writing through to whichever take a frame lands on, which is one edit
becoming an edit of several. Nothing is lost by that -- a join is *replaced*
rather than edited, and re-cutting one costs the list of parts and not the
samples. `take.parts()` is how a view asks before it offers an editable
waveform: the parts when it is a join, an empty list when the buffer owns what
it holds.

## Where a recording lands

A `Buffer` holds a take and a `RecordingStream` follows one as it is written, and
neither puts one in a multitrack. `take` does:

```ts
song.add(form.take(recorded, null, null, { instrument: "player" }), 8.0);
```

It is a `Vector` whose length is the samples' own — frames over the rate they
were recorded at — which is the one line every script used to write by hand.
Without an `instrument` it is structure: it draws and it extends the multitrack, and
it emits no event, which is the `Vector` rule rather than a special case.
