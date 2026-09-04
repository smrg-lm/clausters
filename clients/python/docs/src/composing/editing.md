# Editing on screen: the loop closed

This page is the working rhythm the whole layer was built for, and it has two
beats: **gesture** in the window, **`editor.play()`** to hear the piece as it
now stands. The step in between — folding the gesture into the arrangement —
happens by itself: opening an editor starts the host's [event
loop](../gui.md#the-event-loop-when-nobody-pumps), and the loop applies every
gesture as it arrives. Everything else here is detail on those two.

## The transport, from code

The editor owns a transport over the piece. Give it a destination and a clock
once, and drive it from the interpreter:

```python
editor.locate(0.0)                     # the cursor waits at the top
editor.play(server, session.clock)     # render and play from the cursor
```

The piece plays, and a line sweeps the clips — the **playhead**, anchored to
the engine's own sample clock, so it tracks the audio rather than a guess.
Pause and resume:

```python
editor.pause()      # halt here; the position stays
```

```python
editor.play()       # resume from where we paused (destination remembered)
```

`play` is always a **fresh render** from the transport's position — the tree
is re-flattened, so it plays the composition as it now stands. `pause`
stops the scheduling and keeps the position; what is already sounding finishes
by itself (a transport is not a panic button — every voice in this piece ends
with its clip). And:

```python
editor.stop()        # pause + back to the top
editor.locate(4.0)   # seek: put the transport at beat 4
```

`locate` while stopped just moves the **cursor** — the thin static line the
lanes draw; while playing, it re-renders from the new position (so a seek
also picks up any pending edit). Clicking a lane's **ruler, or its empty
space, is the same `locate`** — try it: click somewhere in the empty part of a
lane and watch the cursor move. That click reaches the data the same way
every other gesture does, which is the next section.

Two playheads, to be precise, and telling them apart matters: the *sweeping
line* is anchored to the engine clock and moves with the audio; the *cursor*
is where a stopped transport sits (where the next `play` starts). You never
set either directly — the transport calls do.

A pass also **ends by itself**. The playhead reports when its scan ran out, so
one call per pass of a script's loop parks the cursor at the composition's end
rather than letting the line sweep off past it. A scan runs out when it renders
its **last item**, and the last clip is still sounding then — so the line goes
on crossing it and the cursor parks only when it reaches the end:

```python
while editor.window is not None:
    editor.transport.update()          # the piece ended: park the cursor
    time.sleep(0.05)
```

The end it parks at is `editor.extent()`, read from the arrangement each time —
drag a clip past the end and the piece is longer, so that is where it now stops.

`editor.transport` is a `clausters.gui.Transport` (the [API
reference](../api.md)), the shared machinery behind play/pause/stop/locate; the
editor's own calls
delegate to it. Every view that shows a playhead drives the same object — the
multitrack's lanes, a piano-roll, an engraved page — because none of it is about
what the view draws: it is one anchor number for the host to sweep from, plus
the position the next play starts at.

## The rhythm: gesture → `play()`

A gesture in the window reaches the arrangement on its own: the editor is
subscribed to the host's event loop, which applies each one as it arrives. Run
the whole cycle once: **drag the second drums clip** somewhere later in its lane
(it snaps to the half-beat grid, your `quant`), then:

```python
print(drums.members)   # the second take's offset is where you dropped it
```

The drag became an `Aggregate.move` on the member handle — the same edit you
made in code two pages ago, arriving through the window. Hear it:

```python
editor.play(at=0.0)
```

**Resize** works the same way: drag a clip's *edge* (the outer few pixels) and
its placement gets a `dur` — the trim rule from the grouping page, by hand.
Pull the second take's right edge in to one beat, then:

```python
print(drums.members)          # (offset, 1.0, <Vector>) — trimmed
print(take.to_event()["dur"]) # 2.0 — the element, untouched as ever
editor.play(at=0.0)           # you hear one beat of it
```

That is the entire loop: the graphic edits the data, the sound plays the data,
and the three never disagree. A few mechanical notes:

- The application of a gesture happens on the loop's thread. To touch a window
  from a **routine** — which must never block the clock thread — hand the work
  to [`app_clock().defer(...)`](../routines-and-clocks.md#the-applications-clock-appclock).
- `editor.poll()` is still there for a script that would rather drain the socket
  itself, and answers `False` while the loop is the one delivering.
- The edit-back arrives in timeline samples; the editor converts to beats and
  snaps to `quant` — the same grid the lane snapped the drag to, so the round
  trip is exact. A drag that moved less than half a sample is not an edit.
- Only what actually changed is written: a plain drag carries the clip's
  length along unchanged, and the editor is careful not to re-snap *that* —
  snapping a length that was never touched would silently shorten the
  element.

## `dirty`, and the `follow` variant

An edit does not interrupt what is sounding. It changes the arrangement and
marks the editor:

```python
print(editor.dirty)    # True after an applied edit, until the next render
```

The next transport action — a play, a resume, a seek — re-reads the
composition, because rendering always re-flattens. If you want the piece to
re-schedule *itself* on every edit instead, turn on **follow**:

```python
editor.follow = True   # the live editor: an edit re-renders on the spot
```

With `follow` on, the rhythm loses its second step too: gesture, and it already
plays from the playhead's position. The semantics are honest —
**re-schedule from here**, not a sample-exact splice: a synth already sounding
keeps sounding, and what changes is what has not been scheduled yet.

```python
editor.follow = False  # back to the explicit `play()` for the next pages
```

## The piece ends where the arrangement says

Let the piece play to the end: the playhead reaches `editor.extent()` — read
from the tree, remember — and the scan simply runs out of events. Drag a clip
past the old end and `play()`: the piece is longer now, and the
playhead sweeps to the *new* end. Nothing was configured; the length is not a
setting, it is a fact about the elements.

Undo, by the way, is a placement again: you watched every edit land as one
(`drums.members`), so putting one back is a `move` — from code or by dragging
it home.

Next: [Automation: a curve as an element](automation.md).
