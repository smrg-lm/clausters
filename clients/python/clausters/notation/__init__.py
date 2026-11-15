"""The symbolic score: notation as data, edited and engraved with no window.

A **symbolic score** is notation -- staves, notes and their engraving, held as
MEI -- and not an OSC score (`clausters.OscScore`, the timed bundles an offline
render runs). Everything here works in a script that opens no window and boots
no GUI host: a score is read, edited, written, read back into sound and
engraved into a display list through the C ABI alone. The score editor is the
application over the same `Score` (`clausters.gui.editing.ScoreEditor`), and
putting a page on screen is the GUI's (`clausters.gui.score_view`,
`clausters.gui.page_json`, `clausters.gui.PlayheadSync.for_score`).

The whole layer is **native and shared**: the engraver is libverovio, bound in
Rust (``clausters-verovio``), and the format-agnostic parts -- the score model,
the SVG-to-display-list walk, the MEI writer, the timemap-to-cursor fold -- live
in ``clausters-notation``, reached here through the C ABI (`clausters._native`).
This module is the Python shell over that: idiomatic names, dicts and a handle
whose lifetime Python owns. A second client in another language rebinds the
same ABI instead of reimplementing any of it.

The library ships **inside the wheel** (``clausters/_libs``) exactly as the Faust
compiler and its LLVM do: an installed package engraves with nothing else on the
machine. In a source checkout, build libverovio with
``third_party/build-verovio.sh``, build the ABI with the ``verovio`` feature on,
and stage both with ``build_native.py``.

There are three ways in: typed score text (ABC/PAE/MEI/MusicXML) handed to
`engrave`/`Score`; `from_notes`/`from_timeline`, which turn the client's own
`clausters.seq` data into MEI (the inverse direction, data->score); and
`svg_to_display_list`, the adapter the first two both flow through.

**Module layout.** The layer is split by what each part knows: `engraver` holds
the score and its engraving (`Score`, `engrave`, `svg_to_display_list`), `mei`
is the pair of reductions between the client's own sequencing data and a score,
in both directions (`from_timeline`, `to_timeline`), and `sheet` is the **score
model** -- notation as data, operations as data over it, and the reading that
turns it back into sound (`to_notes`, `interpretation`). Every name is
re-exported here.
"""

from .engraver import Score, engrave, svg_to_display_list
from .mei import (
    TRANSCRIPTION_KEYS, from_notes, from_timeline, sheet_from_events,
    sheet_from_notes, sheet_from_timeline, to_sequence, to_timeline,
)
from .sheet import (
    add_control, add_spanner, apply, concat, delete, enter, header, insert, insert_measures,
    interpretation, invert, item_id, marks, measures, move_steps, ops, pitch,
    remove_measures,
    read_events, remove_control, remove_spanner, render_events, repeat, retrograde, set_barline, set_break,
    set_clef, set_dur, set_ending, set_groups, set_key, set_mark, set_multirests, set_repeat,
    set_staff,
    set_header, set_marks, set_meter, set_page, set_pitches, silence, stack,
    stretch, tie,
    to_mei, to_notes, to_voice, transpose,
)
from .sheet import from_mei as sheet_from_mei
from .sheet import from_voice as sheet_from_voice

__all__ = [
    "set_staff",
    "set_repeat",
    "set_multirests",
    "set_mark",
    "set_key",
    "set_groups",
    "set_ending",
    "set_clef",
    "remove_control",
    "add_control",
    "Score",
    "add_spanner",
    "apply",
    "concat",
    "delete",
    "engrave",
    "enter",
    "from_notes",
    "from_timeline",
    "header",
    "insert",
    "insert_measures",
    "interpretation",
    "invert",
    "item_id",
    "marks",
    "measures",
    "move_steps",
    "ops",
    "pitch",
    "remove_measures",
    "remove_spanner",
    "repeat",
    "retrograde",
    "set_barline",
    "set_break",
    "set_dur",
    "set_header",
    "set_marks",
    "set_meter",
    "set_page",
    "set_pitches",
    "read_events",
    "render_events",
    "sheet_from_events",
    "TRANSCRIPTION_KEYS",
    "sheet_from_mei",
    "sheet_from_notes",
    "sheet_from_timeline",
    "sheet_from_voice",
    "silence",
    "stack",
    "stretch",
    "svg_to_display_list",
    "tie",
    "to_mei",
    "to_notes",
    "to_timeline",
    "to_sequence",
    "to_voice",
    "transpose",
]

