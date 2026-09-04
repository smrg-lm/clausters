# Composing a piece, step by step

This section builds one short piece from nothing, using the **arrangement**
(`clausters.form`) and its **multitrack editor** (`clausters.gui.FormEditor`), and it
builds it the way the pair is meant to be used: **interactively**. You keep one Python interpreter open from the first
page to the last — IPython, or an editor that evaluates `# %%` cells — and
every block below assumes the names defined by the blocks before it. Evaluate
a block, hear (or see) its result, move on.

[Composition: the arrangement and the multitrack editor](composition.md) is the
companion chapter: it explains *why* the layer is shaped the way it is. This
section is the *doing* — each concept appears when the piece needs it, with a
way to probe it on the spot. When you want the deeper reasoning behind a rule,
follow the links back to that chapter.

## The one loop

Everything here teaches a single working loop:

```text
data  ⟷  graphic  ⟷  sound
```

You write **elements** in Python and place them in time (data). The editor
**draws** them as tracks of clips (graphic). Playing **renders** them (sound).
And the arrows point both ways: dragging a clip on screen edits the *element* —
not a picture of it — so the next play is the piece as you left it, and a change
you make in code redraws and replays the same way. The graphic is a view of the
data; the sound is a render of the data; the data is the piece.

Concretely, the rhythm you will fall into on the editor pages is:

```python
# make a gesture in the window (drag a clip, reshape a curve) ... then:
editor.play()      # hear the piece as it now stands
```

No polling loop, no dispatch table, and no step in between: opening a window
starts the host's [event loop](gui.md#the-event-loop-and-the-one-call-a-script-ends-with), which
applies each gesture to the arrangement as it arrives, so the only call left is
the one that plays the result. (A self-contained *script* form of the very same
piece — transport buttons on screen — is `examples/editors/composer.py`; the
tutorial points at it at the end.)

## What you will build

A four-lane piece, eight beats long:

- **drums** — an audio *take*, bounced offline and loaded from disk, placed
  twice (a `Vector` element);
- **bass** — a `Pbind` pattern: a *generator* the editor bounces to draw and
  the render bounces to play (a `Sequence` element);
- **lead** — a melody of events placed by hand on a timeline (a `Track`
  element);
- **sweep** — a frequency envelope *and the voice it drives*, grouped so they
  are one clip (an `Automation` inside an `Aggregate`).

Along the way you will play elements alone, inspect the flattened score,
open the piece as a multitrack window, move clips by hand and hear the edits,
reshape the automation curve in place, wire a small signal graph and rewire
it on its patch, and finally bounce the piece to a file — sample-identical to
what you heard.

## The pages

1. [Setup: a session, two instruments, a take](composing/setup.md) — the live
   session, the defs the piece needs, and a take bounced offline.
2. [Elements: the five primitives](composing/elements.md) — what an element
   is, and the piece's elements built one by one.
3. [Grouping: placing elements in time](composing/grouping.md) — the
   `Aggregate`, placements, and the song tree.
4. [Rendering: hearing the arrangement](composing/render.md) — flattening,
   the playhead, and why every play re-reads the tree.
5. [The multitrack editor: the arrangement on screen](composing/editor.md) — the
   window, the mapping rules, and the unit bridge.
6. [Editing on screen: the loop closed](composing/editing.md) — the transport,
   the gesture → `poll()` → `play()` rhythm, and `follow`.
7. [Automation: a curve as an element](composing/automation.md) — the sweep, the
   layered clip, and editing the curve in place.
8. [The logical side: groups as signal graphs](composing/logical.md) — the
   other grouping kind, and the patcher.
9. [Bouncing: the piece as a file](composing/bounce.md) — the same tree,
   offline.
10. [Glossary](composing/glossary.md) — every term this section uses, pinned.

## Two units, stated up front

One conversion runs under the whole section, so meet it before it can
surprise you. The **arrangement places elements in beats** — musical,
tempo-relative time. The **view places clips in timeline samples** — one unit per
audio sample, so an audio take sits 1:1 on the axis. The editor is the *only*
converter between the two: one beat is `sample_rate / tempo` timeline units.
You will see both units in this section — beats whenever you touch the
arrangement, samples only inside the window — and the editor pages make the
bridge explicit.

## Prerequisites

A working install ([Getting started](getting-started.md)), a display and a GPU
adapter for the editor pages, and passing familiarity with defs
([Defining instruments](defs.md)) and with patterns and timelines
([Timelines and the playhead](timelines.md)) — the piece uses a `Pbind`, a
`Timeline` and two small `SynthDef`s, all shown in full when they appear.
