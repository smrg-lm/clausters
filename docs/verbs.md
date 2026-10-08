# The verbs, and the client API each one is

A verb is what a window answers — a menu entry, a key, a tool all name one —
and the verbs are written once, in `crates/clausters-editing/src/verbs.rs`:
each one's name, its default keys and its words. A menu is a view over them
and so is a key; **neither is the functionality**. Every verb an application
answers is also something a client's handler can do without a window in the
way, and in both clients the same way. This file is where that is written down.

It is what `docs/gui-props.md` is for the widget props: it does not require
every verb to have a client API yet — it requires that every verb's state be
**decided**, and that the two clients agree. `clients/python/tests/test_verbs.py`
reads the three surfaces and fails when one of them differs from a row:

- **the verb table** is read statically from `verbs.rs`, scope by scope, and the
  rows here are exactly its rows — a verb added there and not here fails;
- **the Python client** is read by importing it: each member a row names exists
  on that class;
- **the web client** is read statically from `clients/web/src`: each member
  exists in that class's file, spelled as the Python one in camelCase.

## How to read a row

The two client columns name the members a handler calls for the verb —
`Class.member`, several when the verb belongs to several editors — or `—`.
The verdict is one of three:

- **`api`** — both clients have it, under the names given. A note says how,
  when the member is not simply the verb (a property set, an argument).
- **`n/a`** — nothing for a client to call, with the reason.
- **`gap`** — the verb is the window's and no client can ask for it without
  the window: work waiting, in both clients at once (`crates/clausters-apps/PLAN.md`,
  Found by use, "Most of the windows' verbs have no client API"). A row is never
  half a gap: one client having a verb the other lacks is the divergence the
  test fails on.

| scope | verb | Python | web | verdict |
|---|---|---|---|---|
| `window` | `undo` | `Editor.undo` | `Editor.undo` | **api** |
| `window` | `redo` | `Editor.redo` | `Editor.redo` | **api** |
| `window` | `save` | `AudioEditor.save` `ScoreEditor.save` | `AudioEditor.save` `ScoreEditor.save` | **api** |
| `window` | `close` | `Editor.close` | `Editor.close` | **api** |
| `window` | `view_all` | — | — | **gap** |
| `window` | `play` | `MultitrackEditor.play` `NotesEditor.play` `ScoreEditor.play` | `MultitrackEditor.play` `NotesEditor.play` `ScoreEditor.play` | **api** |
| `window` | `loop` | — | — | **gap** |
| `window` | `to_start` | `MultitrackEditor.rewind` | `MultitrackEditor.rewind` | **api** — `rewind` puts the position cursor back at the top |
| `window` | `to_end` | — | — | **gap** |
| `window` | `copy` | — | — | **gap** |
| `window` | `cut` | — | — | **gap** |
| `window` | `paste` | — | — | **gap** |
| `window` | `mix` | — | — | **gap** |
| `window` | `quantize` | — | — | **gap** |
| `window` | `split` | — | — | **gap** |
| `window` | `join` | — | — | **gap** |
| `window` | `delete` | — | — | **gap** |
| `window` | `select_all` | — | — | **gap** |
| `window` | `keys` | — | — | **n/a** — the window's key sheet, a page of the window itself rather than an act on what it holds |
| `multitrack` | `pause` | `MultitrackEditor.pause` | `MultitrackEditor.pause` | **api** |
| `multitrack` | `stop` | `MultitrackEditor.stop` | `MultitrackEditor.stop` | **api** |
| `multitrack` | `stop_at_end` | — | — | **gap** |
| `multitrack` | `crossfade` | `Multitrack.crossfade` | `Multitrack.crossfade` | **api** — a default of the multitrack, set on the structure |
| `multitrack` | `add_track` | `Tracks.add` | `Tracks.add` | **api** — on the structure: `multitrack.tracks.add()` |
| `multitrack` | `reset_heights` | — | — | **gap** |
| `multitrack` | `compact_tracks` | — | — | **gap** |
| `multitrack` | `notes_roll` | `MultitrackEditor.notes_view` | `MultitrackEditor.notesView` | **api** — `notes_view = "roll"` |
| `multitrack` | `notes_score` | `MultitrackEditor.notes_view` | `MultitrackEditor.notesView` | **api** — `notes_view = "score"` |
| `score` | `entry` | `ScoreEditor.entry` | `ScoreEditor.entry` | **api** |
| `score` | `delete` | `ScoreEditor.delete` | `ScoreEditor.delete` | **api** |
| `score` | `deselect` | `ScoreEditor.select` | `ScoreEditor.select` | **api** — with no elements |
| `score` | `select_left` | — | — | **gap** |
| `score` | `select_right` | — | — | **gap** |
| `score` | `step_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `score` | `step_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `score` | `octave_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `score` | `octave_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `note_entry` | `entry` | `ScoreEditor.entry` | `ScoreEditor.entry` | **api** |
| `note_entry` | `entry_off` | `ScoreEditor.entry` | `ScoreEditor.entry` | **api** — `entry = False` |
| `note_entry` | `pitch_a` | — | — | **gap** |
| `note_entry` | `pitch_b` | — | — | **gap** |
| `note_entry` | `pitch_c` | — | — | **gap** |
| `note_entry` | `pitch_d` | — | — | **gap** |
| `note_entry` | `pitch_e` | — | — | **gap** |
| `note_entry` | `pitch_f` | — | — | **gap** |
| `note_entry` | `pitch_g` | — | — | **gap** |
| `note_entry` | `chord_a` | — | — | **gap** |
| `note_entry` | `chord_b` | — | — | **gap** |
| `note_entry` | `chord_c` | — | — | **gap** |
| `note_entry` | `chord_d` | — | — | **gap** |
| `note_entry` | `chord_e` | — | — | **gap** |
| `note_entry` | `chord_f` | — | — | **gap** |
| `note_entry` | `chord_g` | — | — | **gap** |
| `note_entry` | `cursor_left` | — | — | **gap** |
| `note_entry` | `cursor_right` | — | — | **gap** |
| `note_entry` | `bar_left` | — | — | **gap** |
| `note_entry` | `bar_right` | — | — | **gap** |
| `note_entry` | `staff_up` | — | — | **gap** |
| `note_entry` | `staff_down` | — | — | **gap** |
| `note_entry` | `step_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `note_entry` | `step_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `note_entry` | `octave_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `note_entry` | `octave_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** |
| `note_entry` | `voice_1` | — | — | **gap** |
| `note_entry` | `voice_2` | — | — | **gap** |
| `note_entry` | `voice_3` | — | — | **gap** |
| `note_entry` | `voice_4` | — | — | **gap** |
| `note_entry` | `value_64th` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `value_32nd` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `value_16th` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `value_eighth` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `value_quarter` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `value_half` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `value_whole` | `ScoreEditor.value` | `ScoreEditor.value` | **api** |
| `note_entry` | `dot` | `ScoreEditor.dotted` | `ScoreEditor.dotted` | **api** |
| `note_entry` | `enter_rest` | — | — | **gap** |
