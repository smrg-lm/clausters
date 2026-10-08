# The verbs, and the client API each one is

A verb is what a window answers — a menu entry, a key, a tool all name one —
and the verbs are written once, in `crates/clausters-editing/src/verbs.rs`:
each one's name, its default keys and its words. A menu is a view over them
and so is a key; **neither is the functionality**. Every verb an application
answers is also something a client's handler can do, and in both clients the
same way: **`editor.verb(name)`** asks the host for any verb of the table on
the editor's window (`/gui_verb`, through the same dispatch a key and a tool
end in), and a **named member** says it with a name and arguments of its
own — `editor.split()`, `editor.write("C")`. The named members are the
UGen builders of this layer: one line each over the one door, so the logic
stays in the host and the table, and a verb added there is reachable through
`verb` before anyone writes a member for it. This file is where the named
members are written down.

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
- **`gap`** — no named member yet, in either client: the verb is still
  reachable through `editor.verb`, and a member is work waiting in both clients
  at once. None today. A row is never half a gap: one client having a member
  the other lacks is the divergence the test fails on.

| scope | verb | Python | web | verdict |
|---|---|---|---|---|
| `window` | `undo` | `Editor.undo` | `Editor.undo` | **api** |
| `window` | `redo` | `Editor.redo` | `Editor.redo` | **api** |
| `window` | `save` | `AudioEditor.save` `ScoreEditor.save` | `AudioEditor.save` `ScoreEditor.save` | **api** |
| `window` | `close` | `Editor.close` | `Editor.close` | **api** |
| `window` | `view_all` | `Editor.view_all` | `Editor.viewAll` | **api** |
| `window` | `play` | `MultitrackEditor.play` `NotesEditor.play` `ScoreEditor.play` | `MultitrackEditor.play` `NotesEditor.play` `ScoreEditor.play` | **api** |
| `window` | `loop` | `Editor.loop` | `Editor.loop` | **api** |
| `window` | `to_start` | `Editor.to_start` | `Editor.toStart` | **api** — the multitrack's `rewind` also cues a stopped transport there |
| `window` | `to_end` | `Editor.to_end` | `Editor.toEnd` | **api** |
| `window` | `copy` | `Editor.copy` | `Editor.copy` | **api** |
| `window` | `cut` | `Editor.cut` | `Editor.cut` | **api** |
| `window` | `paste` | `Editor.paste` | `Editor.paste` | **api** |
| `window` | `mix` | `AudioEditor.mix` | `AudioEditor.mix` | **api** |
| `window` | `quantize` | `MultitrackEditor.quantize` `NotesEditor.quantize` | `MultitrackEditor.quantize` `NotesEditor.quantize` | **api** |
| `window` | `split` | `MultitrackEditor.split` `NotesEditor.split` | `MultitrackEditor.split` `NotesEditor.split` | **api** |
| `window` | `join` | `MultitrackEditor.join` `NotesEditor.join` | `MultitrackEditor.join` `NotesEditor.join` | **api** |
| `window` | `snap` | `MultitrackEditor.snap_to_grid` `NotesEditor.snap_to_grid` | `MultitrackEditor.snapToGrid` `NotesEditor.snapToGrid` | **api** — a switch: `snap_to_grid = False` |
| `window` | `delete` | `Editor.delete` | `Editor.delete` | **api** |
| `window` | `select_all` | `Editor.select_all` | `Editor.selectAll` | **api** |
| `window` | `keys` | — | — | **n/a** — the window's key sheet, a page of the window itself rather than an act on what it holds; `verb("keys")` shows it |
| `multitrack` | `pause` | `MultitrackEditor.pause` | `MultitrackEditor.pause` | **api** |
| `multitrack` | `stop` | `MultitrackEditor.stop` | `MultitrackEditor.stop` | **api** |
| `multitrack` | `stop_at_end` | `MultitrackEditor.stop_at_end` | `MultitrackEditor.stopAtEnd` | **api** |
| `multitrack` | `crossfade` | `Multitrack.crossfade` | `Multitrack.crossfade` | **api** — a default of the multitrack, set on the structure |
| `multitrack` | `add_track` | `Tracks.add` | `Tracks.add` | **api** — on the structure: `multitrack.tracks.add()` |
| `multitrack` | `reset_heights` | `MultitrackEditor.reset_heights` | `MultitrackEditor.resetHeights` | **api** |
| `multitrack` | `compact_tracks` | `MultitrackEditor.compact_tracks` | `MultitrackEditor.compactTracks` | **api** |
| `multitrack` | `notes_roll` | `MultitrackEditor.notes_view` | `MultitrackEditor.notesView` | **api** — `notes_view = "roll"` |
| `multitrack` | `notes_score` | `MultitrackEditor.notes_view` | `MultitrackEditor.notesView` | **api** — `notes_view = "score"` |
| `score` | `entry` | `ScoreEditor.entry` | `ScoreEditor.entry` | **api** |
| `score` | `delete` | `ScoreEditor.delete` | `ScoreEditor.delete` | **api** |
| `score` | `deselect` | `ScoreEditor.select` | `ScoreEditor.select` | **api** — with no elements |
| `score` | `select_left` | `ScoreEditor.select_left` | `ScoreEditor.selectLeft` | **api** |
| `score` | `select_right` | `ScoreEditor.select_right` | `ScoreEditor.selectRight` | **api** |
| `score` | `step_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(1)` |
| `score` | `step_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(-1)` |
| `score` | `octave_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(7)` |
| `score` | `octave_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(-7)` |
| `note_entry` | `entry` | `ScoreEditor.entry` | `ScoreEditor.entry` | **api** |
| `note_entry` | `entry_off` | `ScoreEditor.entry` | `ScoreEditor.entry` | **api** — `entry = False` |
| `note_entry` | `pitch_a` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("A")` |
| `note_entry` | `pitch_b` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("B")` |
| `note_entry` | `pitch_c` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("C")` |
| `note_entry` | `pitch_d` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("D")` |
| `note_entry` | `pitch_e` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("E")` |
| `note_entry` | `pitch_f` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("F")` |
| `note_entry` | `pitch_g` | `ScoreEditor.write` | `ScoreEditor.write` | **api** — `write("G")` |
| `note_entry` | `chord_a` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("A")` |
| `note_entry` | `chord_b` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("B")` |
| `note_entry` | `chord_c` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("C")` |
| `note_entry` | `chord_d` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("D")` |
| `note_entry` | `chord_e` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("E")` |
| `note_entry` | `chord_f` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("F")` |
| `note_entry` | `chord_g` | `ScoreEditor.chord` | `ScoreEditor.chord` | **api** — `chord("G")` |
| `note_entry` | `cursor_left` | `ScoreEditor.cursor_left` | `ScoreEditor.cursorLeft` | **api** |
| `note_entry` | `cursor_right` | `ScoreEditor.cursor_right` | `ScoreEditor.cursorRight` | **api** |
| `note_entry` | `bar_left` | `ScoreEditor.bar_left` | `ScoreEditor.barLeft` | **api** |
| `note_entry` | `bar_right` | `ScoreEditor.bar_right` | `ScoreEditor.barRight` | **api** |
| `note_entry` | `staff_up` | `ScoreEditor.staff_up` | `ScoreEditor.staffUp` | **api** |
| `note_entry` | `staff_down` | `ScoreEditor.staff_down` | `ScoreEditor.staffDown` | **api** |
| `note_entry` | `step_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(1)` |
| `note_entry` | `step_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(-1)` |
| `note_entry` | `octave_up` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(7)` |
| `note_entry` | `octave_down` | `ScoreEditor.move` | `ScoreEditor.move` | **api** — `move(-7)` |
| `note_entry` | `voice_1` | `ScoreEditor.entry_voice` | `ScoreEditor.entryVoice` | **api** — `entry_voice(1)` |
| `note_entry` | `voice_2` | `ScoreEditor.entry_voice` | `ScoreEditor.entryVoice` | **api** — `entry_voice(2)` |
| `note_entry` | `voice_3` | `ScoreEditor.entry_voice` | `ScoreEditor.entryVoice` | **api** — `entry_voice(3)` |
| `note_entry` | `voice_4` | `ScoreEditor.entry_voice` | `ScoreEditor.entryVoice` | **api** — `entry_voice(4)` |
| `note_entry` | `value_64th` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 64)` |
| `note_entry` | `value_32nd` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 32)` |
| `note_entry` | `value_16th` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 16)` |
| `note_entry` | `value_eighth` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 8)` |
| `note_entry` | `value_quarter` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 4)` |
| `note_entry` | `value_half` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 2)` |
| `note_entry` | `value_whole` | `ScoreEditor.value` | `ScoreEditor.value` | **api** — `value = (1, 1)` |
| `note_entry` | `dot` | `ScoreEditor.dotted` | `ScoreEditor.dotted` | **api** |
| `note_entry` | `enter_rest` | `ScoreEditor.enter_rest` | `ScoreEditor.enterRest` | **api** |
