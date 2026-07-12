# Editing on screen: the loop closed

This page is the working rhythm the whole layer was built for. It has three
beats: **gesture** in the window, **`editor.poll()`** to fold the gesture into
the model, **`editor.play()`** to hear the piece as it now stands. Everything
else here is detail on those three.

## The transport, from code

The editor owns a transport over the model. Give it a destination and a clock
once, and drive it from the interpreter:

```python
editor.locate(0.0)                     # the cursor waits at the top
editor.play(server, session.clock)     # realize and play from the cursor
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

`play` is always a **fresh realization** from the transport's position — the
model is re-flattened, so it plays the composition as it now stands. `pause`
stops the scheduling and keeps the position; what is already sounding finishes
by itself (a transport is not a panic button — every voice in this piece ends
with its clip). And:

```python
editor.stop()        # pause + back to the top
editor.locate(4.0)   # seek: put the transport at beat 4
```

`locate` while stopped just moves the **cursor** — the thin static line the
lanes draw; while playing, it re-realizes from the new position (so a seek
also picks up any pending edit). Clicking a lane's **ruler, or its empty
space, is the same `locate`** — try it: click somewhere in the empty part of a
lane and watch the cursor move. That click reaches the model the same way
every other gesture does, which is the next section.

Two playheads, to be precise, and telling them apart matters: the *sweeping
line* is anchored to the engine clock and moves with the audio; the *cursor*
is where a stopped transport sits (where the next `play` starts). You never
set either directly — the transport calls do.

## The rhythm: gesture → `poll()` → `play()`

The window accumulates your gestures as events; nothing touches the model
until you ask. `editor.poll()` drains everything pending and applies it — one
call, no loop. Run the whole cycle once: **drag the second drums clip**
somewhere later in its lane (it snaps to the half-beat grid, your `quant`),
then:

```python
editor.poll()          # -> True: the composition changed
print(drums.members)   # the second take's offset is where you dropped it
```

The drag became a `Group.move` on the member handle — the same edit you made
in code two pages ago, arriving through the window. Hear it:

```python
editor.play(at=0.0)
```

**Resize** works the same way: drag a clip's *edge* (the outer few pixels) and
its placement gets a `dur` — the trim rule from the grouping page, by hand.
Pull the second take's right edge in to one beat, then:

```python
editor.poll()
print(drums.members)          # (offset, 1.0, <Buffer>) — trimmed
print(take.to_event()["dur"]) # 2.0 — the material, untouched as ever
editor.play(at=0.0)           # you hear one beat of it
```

That is the entire loop: the graphic edits the data, the sound plays the data,
and the three never disagree. A few mechanical notes:

- `poll()` returns whether the composition changed, and it is safe to call any
  time — unknown messages are ignored, a closed window just marks itself.
- Call it from the interpreter (or your own loop), **never from the clock
  thread** — the same golden rule as every routine.
- The edit-back arrives in timeline samples; the editor converts to beats and
  snaps to `quant` — the same grid the lane snapped the drag to, so the round
  trip is exact. A drag that moved less than half a sample is not an edit.
- Only what actually changed is written: a plain drag carries the clip's
  length along unchanged, and the editor is careful not to re-snap *that* —
  snapping a length that was never touched would silently shorten the
  material.

## `dirty`, and the `follow` variant

An edit does not interrupt what is sounding. It changes the model and marks
the editor:

```python
print(editor.dirty)    # True after an applied edit, until the next realize
```

The next transport action — a play, a resume, a seek — re-reads the
composition, because realizing always re-flattens. If you want the piece to
re-schedule *itself* on every edit instead, turn on **follow**:

```python
editor.follow = True   # the live editor: poll() now re-realizes on each edit
```

With `follow` on, the same rhythm loses its third step: gesture, `poll()`,
and it already plays from the playhead's position. The semantics are honest —
**re-schedule from here**, not a sample-exact splice: a synth already sounding
keeps sounding, and what changes is what has not been scheduled yet.

```python
editor.follow = False  # back to the explicit rhythm for the next pages
```

## The piece ends where the model says

Let the piece play to the end: the playhead reaches `editor.extent()` — read
from the model, remember — and the scan simply runs out of events. Drag a clip
past the old end, `poll()`, `play()`: the piece is longer now, and the
playhead sweeps to the *new* end. Nothing was configured; the length is not a
setting, it is a fact about the material.

Undo, by the way, is the model again: you watched every edit land as a
placement (`drums.members`), so putting one back is a `move` — from code or by
dragging it home.

Next: [Automation: a curve as material](automation.md).
