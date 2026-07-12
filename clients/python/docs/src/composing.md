# Composing a piece, step by step

This section builds one short piece from nothing, using the **arrangement
model** (`clausters.model`) and its **multitrack editor**
(`clausters.gui.Editor`), and it builds it the way the pair is meant to be
used: **interactively**. You keep one Python interpreter open from the first
page to the last — IPython, or an editor that evaluates `# %%` cells — and
every block below assumes the names defined by the blocks before it. Evaluate
a block, hear (or see) its result, move on.

[Composition: the model and the multitrack editor](composition.md) is the
companion chapter: it explains *why* the layer is shaped the way it is. This
section is the *doing* — each concept appears when the piece needs it, with a
way to probe it on the spot. When you want the deeper reasoning behind a rule,
follow the links back to that chapter.

## The one loop

Everything here teaches a single working loop:

```text
data  ⟷  graphic  ⟷  sound
```

You write **materials** in Python and place them in time (data). The editor
renders them as tracks of clips (graphic). Playing **realizes** the model
(sound). And the arrows point both ways: dragging a clip on screen edits the
*material* — not a picture of it — so the next play is the piece as you left
it, and a change you make in code redraws and replays the same way. The
graphic is a view of the data; the sound is a realization of the data; the
data is the piece.

Concretely, the rhythm you will fall into on the editor pages is:

```python
# make a gesture in the window (drag a clip, reshape a curve) ... then:
editor.poll()      # fold the pending edits into the model
editor.play()      # hear the piece as it now stands
```

No polling loop, no dispatch table: one call drains the window's pending
events into the model, and one call plays the result. (A self-contained
*script* form of the very same piece — transport buttons on screen, a poll
loop — is `examples/gui_composer.py`; the tutorial points at it at the end.)

## What you will build

A four-lane piece, eight beats long:

- **drums** — an audio *take*, bounced offline and loaded from disk, placed
  twice (a `Buffer` material);
- **bass** — a `Pbind` pattern: a *generator* the editor bounces to draw and
  the realization bounces to play (a `Sequence` material);
- **lead** — a melody of events placed by hand on a timeline (a `Track`
  material);
- **sweep** — a frequency envelope *and the voice it drives*, grouped so they
  are one clip (an `Automation` inside a `Group`).

Along the way you will play materials alone, inspect the flattened score,
open the piece as a multitrack window, move clips by hand and hear the edits,
reshape the automation curve in place, wire a small signal graph and rewire
it on its patch, and finally bounce the piece to a file — sample-identical to
what you heard.

## The pages

1. [Setup: a session, two instruments, a take](composing/setup.md) — the live
   session, the defs the piece needs, and a take bounced offline.
2. [Materials: the five primitives](composing/materials.md) — what a material
   is, and the piece's materials built one by one.
3. [Grouping: placing materials in time](composing/grouping.md) — the `Group`,
   placements, and the song tree.
4. [Realization: hearing the model](composing/realization.md) — flattening,
   the playhead, and why every play re-reads the model.
5. [The multitrack editor: the model on screen](composing/editor.md) — the
   window, the mapping rules, and the unit bridge.
6. [Editing on screen: the loop closed](composing/editing.md) — the transport,
   the gesture → `poll()` → `play()` rhythm, and `follow`.
7. [Automation: a curve as material](composing/automation.md) — the sweep, the
   layered clip, and editing the curve in place.
8. [The logical side: groups as signal graphs](composing/logical.md) — the
   other grouping kind, and the patcher.
9. [Bouncing: the piece as a file](composing/bounce.md) — the same model,
   offline.
10. [Glossary](composing/glossary.md) — every term this section uses, pinned.

## Two units, stated up front

One conversion runs under the whole section, so meet it before it can
surprise you. The **model places materials in beats** — musical, tempo-relative
time. The **view places clips in timeline samples** — one unit per audio
sample, so an audio take sits 1:1 on the axis. The editor is the *only*
converter between the two: one beat is `sample_rate / tempo` timeline units.
You will see both units in this section — beats whenever you touch the model,
samples only inside the window — and the editor pages make the bridge
explicit.

## Prerequisites

A working install ([Getting started](getting-started.md)), a display and a GPU
adapter for the editor pages, and passing familiarity with defs
([Defining instruments](defs.md)) and with patterns and timelines
([Timelines and the playhead](timelines.md)) — the piece uses a `Pbind`, a
`Timeline` and two small `SynthDef`s, all shown in full when they appear.
