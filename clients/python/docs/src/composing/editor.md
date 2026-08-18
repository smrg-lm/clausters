# The multitrack editor: the arrangement on screen

`Editor` draws the song tree as a multitrack window — one lane per member of
the root aggregate, each holding its members as clips on one shared time axis —
and, on the next page, applies the window's edits back onto that tree. It is
the one module that knows both worlds: `clausters.form` never imports the GUI.

## Open the piece

```python
from clausters.gui import Editor

gui = session.gui()      # boot (once) the visual server, wired to this session
editor = Editor(song, sample_rate=SR, tempo=TEMPO, quant=QUANT,
                title="Composer")
win = editor.open(gui)
print(f"opened window {win}")
```

Three lanes appear — *drums*, *bass*, *lead* — with a beats ruler under the
bottom one. What each argument fixed:

- `sample_rate` and `tempo` close the **unit bridge** (below);
- `quant` is the musical drag grid, in beats — `0.5` means clips will snap to
  half-beats, both on screen and in the data;
- the window is `title`d, and the editor allocates its widget ids from
  `base_id` (default 10 000) up, clear of anything you open yourself.

## Read the view

The mapping from the tree to the window is one rule, not a heuristic per case:

- the **root aggregate's members are the lanes** — top to bottom, in order;
- a **lane's members are its clips**, at their placements on the shared axis;
- a **`Vector` clip draws its take**: the clip names the server buffer, and
  the host fetches it and decimates it to the clip's pixel width — a long
  take costs nothing on the wire;
- an **element of events draws a piano-roll**: each note a bar, high pitches
  up. The bass lane shows its notes too — the *pattern was bounced to draw
  it*, the same change of state that rendering performs, now on screen: a
  generator lane shows the notes it is about to play;
- an **aggregate nested inside a lane draws as a labeled rectangle** — its
  summary — until you expand it. (The root's members are already lanes; the rule is
  about the level below them.)

That last rule is the arrangement's **base level** — the zoom that summarizes
an aggregate or resolves it — and it is an editor call, not a protocol. The
piece has no nested aggregate yet, so make one — a two-note fill dropped into
the lead lane — and watch each step in the window:

```python
from clausters.form import Aggregate, Clang
from clausters.seq.event import Event as SeqEvent

fill = Aggregate([(0.0, Clang(SeqEvent(midinote=84, dur=0.5))),
                  (0.5, Clang(SeqEvent(midinote=88, dur=0.5)))], name="fill")
handle = lead_lane.add(fill, offset=6.0)
editor.update()          # a structural change: push the re-rendered tree
```

The fill appears on the lead lane as one labeled rectangle — the summary of
the level below it. Resolve it, then summarize it back:

```python
editor.expand(fill); editor.update()     # the fill as a lane of its own
```

```python
editor.collapse(fill); editor.update()   # one rectangle again
```

Same structure, seen coarser or finer — the data never changed, only the base
level. The fill was a demonstration, so put the piece back:

```python
lead_lane.remove(handle)
editor.update()
```

`update()` is a whole-tree redefine — the honest way to show a *structural*
change (an element added or removed, an aggregate expanded). You will use it again
on the automation page when the piece grows a lane. A mere placement change
needs no redefine: when you drag a clip, the host already moved it.

## The unit bridge: beats on one side, samples on the other

The arrangement places elements in **beats**. The window places clips in
**timeline samples** — one unit per audio sample, so an audio take sits 1:1 on the axis
and sample 0 of its body is exactly at the clip's left edge. The editor is the
*only* converter between the two, and you gave it everything it needs: one
beat is `sample_rate / tempo` timeline units.

```python
print(editor.units_per_beat)          # == BEAT == SR / TEMPO
print(editor.beats_to_units(2.0))     # where beat 2 sits on the axis
print(editor.units_to_beats(editor.beats_to_units(2.0)))   # 2.0 — round trip
```

The `quant` you passed is a *musical* grid; the editor converts it once and
hands it to the lanes as their drag grid in samples. So the grid a clip snaps
to on screen **is** the grid the arrangement re-schedules on — what you see is
what plays.

## The piece's length is read from the arrangement

```python
print(editor.extent())     # 7.8 — the end of the last sounding event, in beats
```

`extent()` is not a constant: it walks the tree — and it measures what you
*hear*. The last bass note starts at beat 7 and sounds for 0.8 of a beat (its
`dur` scaled by the default `legato`), so the piece is 7.8 beats long, not 8.
Drag a clip past the end (or `move` one there in code) and the piece is longer
— which is exactly what a transport must ask, and what the playhead on the
next page will respect.

## Navigate

All lanes share one time axis, and they navigate as one:

- **mouse wheel** — zoom, toward the cursor;
- **Shift + drag** — pan;
- **`r`** — reset the view to the whole piece;
- **Escape** — close the window.

Those belong to the **axis**, not to the lanes: the same wheel and the same
Shift + drag navigate a waveform, a piano-roll or a free-standing ruler,
because every view that carries a time axis owns them. Which chord does what
is the container's own table and a view can be told to change it — a lane made
to pan on a plain drag with `gestures={"drag": "pan"}` — but the defaults are
what this page describes and the whole editor shares them.

Zoom into the drums take: the waveform re-resolves as you go (the host keeps a
peak pyramid per take and never resolves finer than the screen). Zoom far out:
there is empty time to zoom out into, and the axis spans the longest clip end
over every lane.

The window is open and truthful; now make it *writable*.

Next: [Editing on screen: the loop closed](editing.md).
