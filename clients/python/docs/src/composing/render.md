# Rendering: hearing the arrangement

**Rendering** is the change of state from the arrangement to sound. For a
concrete element it is exactly the walk you ran on the last page, plus a
transport: flatten the tree into a timeline of absolute beats, hand that
timeline to a `Playhead` on a clock, play. Nothing else — no second sequencer
is added; the one the client already has is reused
([Timelines and the playhead](../timelines.md)).

## Play the piece

```python
playhead = song.render(server, session.clock)
```

Eight beats: the take at 0 and 4, the bass walking under everything, the
melody entering at beat 2. What `render` did, in order:

1. `to_timeline(song)` — flatten to absolute beats. Contained generators (the
   bass `Pbind`) are **bounced** in this pass: the pattern runs offline on a
   throwaway clock and its events are recorded at their logical beats. A
   `Sequence` that wraps a plain list of elements is laid out *successively*,
   each after the previous one's duration; an abstract element contributes
   context and no event.
2. `Playhead(timeline, clock, destination)` — a transport over that timeline.
3. `playhead.play(at=0.0)` — the playhead scans forward on the clock, playing
   each item onto the server at its exact logical beat.

The returned playhead *is* the transport, and you can drive it directly:

```python
playhead.stop()                                  # halt the scan
playhead = song.render(server, session.clock, at=4.0)   # from beat 4
print(playhead.position())                       # the song position, in beats
```

`stop` halts the *scheduling* — anything already sounding finishes its own
release. Stopping a playhead is not a panic button, and nothing here needs
one: every voice in this piece is an event with a length, so it ends when its
event does.

Let it run out, or stop it:

```python
playhead.stop()
```

## Rendering always re-reads the tree

There is no cached score. Every `render` re-flattens the tree *as it now
stands*, so an edit in code is heard on the very next play:

```python
member = drums.handles[1]
drums.move(member, 5.0)                          # push the second take late
playhead = song.render(server, session.clock)    # ... and it plays at beat 5
```

```python
playhead.stop()
drums.move(member, 4.0)                          # back where it was
```

This is the property the whole editor rests on. A dragged clip will do exactly
what that `move` did — write a placement — and pressing play will do exactly
what `render` does — re-read the composition. The GUI adds no path of its own.

## The same verb, offline

`render` takes a *destination*, and everything about it is
destination-agnostic: give it an offline session's server and clock and the
identical flattening accumulates a score instead of sounding —
sample-identical to the live playback, because it is the same walk feeding the
same engine. The [bounce page](bounce.md) closes the tutorial with it.

One more dispatch hides in the same verb: a **logical** aggregate (the other
grouping kind) does not flatten at all — `render` sends it to the server as a
signal-graph definition instead. That is [the logical page](logical.md); the
piece does not need it yet.

You now have the whole loop in code: build elements, place them, play them,
edit, play again. Time to put it on screen.

Next: [The multitrack editor: the arrangement on screen](editor.md).
