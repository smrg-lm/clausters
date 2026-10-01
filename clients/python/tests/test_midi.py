"""M17 client sub-part 1: an event pattern rendered as standard MIDI.

A ``Pbind`` played on a ``MidiServer`` destination (the double-dispatch
counterpart of the OSC ``Server``) records note on/off into a ``MidiScore`` in
beats; ``write`` serializes it to a ``.mid`` through the ``clausters-midi``
crate. The score-level test needs no native library; the file test skips if the
cdylib is not built.
"""

import os
import tempfile

import pytest

from clausters.base.timebase import LogicalTimebase
from clausters.base import MidiRtInterface, MidiServer, TempoClock
from clausters.seq import Pbind, Pseq


def _note_ons(events):
    return [(b, m) for b, m in events if (m[0] & 0xF0) == 0x90 and m[2] > 0]


def _note_offs(events):
    return [
        (b, m)
        for b, m in events
        if (m[0] & 0xF0) == 0x80 or ((m[0] & 0xF0) == 0x90 and m[2] == 0)
    ]


def test_pbind_rendered_as_midi_in_beats():
    midi = MidiServer(channel=0)
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
    Pbind(
        instrument="default", midinote=Pseq([60, 64, 67]), dur=0.5, amp=0.5, legato=0.8
    ).play(clock, midi)
    clock.render()

    events = midi.score.sorted()
    assert len(events) == 6  # 3 notes -> 3 on + 3 off

    ons = _note_ons(events)
    assert [b for b, _ in ons] == [0.0, 0.5, 1.0]  # delta = dur*stretch
    assert [m[1] for _, m in ons] == [60, 64, 67]  # note numbers
    assert all(m[2] in (63, 64) for _, m in ons)  # amp 0.5 -> ~64

    offs = _note_offs(events)
    # note off at on + sustain (dur*legato*stretch = 0.5*0.8)
    assert [round(b, 3) for b, _ in offs] == [0.4, 0.9, 1.4]


def test_explicit_freq_maps_to_a_note_number():
    midi = MidiServer()
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
    Pbind(instrument="default", freq=Pseq([440.0]), dur=1.0, amp=1.0).play(clock, midi)
    clock.render()
    on = _note_ons(midi.score.sorted())[0]
    assert on[1][1] == 69  # 440 Hz -> MIDI note 69
    assert on[1][2] == 127  # amp 1.0 -> max velocity


def test_a_midi_item_is_an_event_that_plays_its_own_bytes():
    from clausters.base import OscNrtInterface, Routine
    from clausters.defs import Server
    from clausters.seq import MidiItem, OscItem

    item = MidiItem([0xB0, 74, 40])
    assert (item["type"], item["midicmd"], item["cc"], item["value"]) == ("midi", "cc", 74, 40)

    midi = MidiServer()
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
    clock.play(Routine(lambda: (yield from [item.play(midi)])))
    clock.render()
    assert [(b, list(m)) for b, m in midi.score.sorted()] == [(0.0, [0xB0, 74, 40])]

    with pytest.raises(ValueError, match="no MIDI spelling"):
        OscItem("/x", 1).play(midi)
    with pytest.raises(ValueError, match="plays on a MIDI destination"):
        MidiItem([0x90, 60, 1]).play(Server(interface=OscNrtInterface()))


def _midi_or_skip():
    try:
        from clausters import _midi

        _midi.lib()
    except OSError as e:
        pytest.skip(f"clausters-midi not built: {e}")


def _rendered_midi(**pbind):
    midi = MidiServer()
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
    Pbind(instrument="default", **pbind).play(clock, midi)
    clock.render()
    return midi


def test_write_smf_produces_a_valid_file():
    _midi_or_skip()
    midi = _rendered_midi(midinote=Pseq([60, 67]), dur=1.0, amp=0.6)
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "out.mid")
        midi.write(path, ppq=480)
        data = open(path, "rb").read()
    assert data[:4] == b"MThd"  # SMF header chunk
    assert b"MTrk" in data  # a track chunk
    assert len(data) > 14


def test_write_clip_produces_smf2clip_file():
    _midi_or_skip()
    midi = _rendered_midi(midinote=Pseq([60, 67]), dur=1.0, amp=0.6)
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "out.midiclip")
        midi.write(path, ppq=480, fmt="clip")
        data = open(path, "rb").read()
    assert data[:8] == b"SMF2CLIP"  # MIDI 2.0 clip file header
    assert len(data) > 8


def test_live_output_smoke():
    """Drive a Pbind through a live virtual port (open + send + scheduled
    note-off + close). Skips without the `live` cdylib or a working ALSA seq."""
    try:
        from clausters import _midi

        _midi.lib()
        iface = MidiRtInterface(port="clausters-test")
    except (OSError, RuntimeError) as e:
        pytest.skip(f"live MIDI unavailable: {e}")
    try:
        midi = MidiServer(interface=iface)
        clock = TempoClock(tempo=4.0, timebase=LogicalTimebase())
        Pbind(instrument="default", midinote=Pseq([60, 64, 67]), dur=0.25, amp=0.7).play(
            clock, midi
        )
        clock.render()  # drives the routine: note-ons now, note-offs scheduled
    finally:
        iface.close()


def test_an_mpe_zone_puts_each_note_on_a_member_with_its_expression():
    # Overlapping notes: each takes a member channel of its own, the zone is
    # declared at the head, and a note's bend and pressure precede its note-on.
    midi = MidiServer(zone=3)
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
    Pbind(
        instrument="default", midinote=Pseq([60, 64, 67, 72]), dur=0.5, legato=1.5,
        bend=Pseq([12.0, 0.0, 0.0, 0.0]), press=Pseq([1.0, None, None, None]),
    ).play(clock, midi)
    clock.render()
    events = midi.score.sorted()
    # RPN 6 on the master (channel 0), three members.
    assert events[0] == (0.0, bytes((0xB0, 101, 0)))
    assert events[2] == (0.0, bytes((0xB0, 6, 3)))
    ons = _note_ons(events)
    channels = [m[0] & 0x0F for _, m in ons]
    # Three overlap and take channels 1-3; the fourth reuses the first freed.
    assert channels[:3] == [1, 2, 3]
    assert channels[3] == 1
    # The first note's bend (12 of 48 semitones) and full pressure on its
    # channel, just ahead of its note-on.
    first = [m for b, m in events if b == 0.0 and m[0] & 0x0F == 1]
    assert first[0][0] == 0xE1 and ((first[0][2] << 7) | first[0][1]) == 8192 + round(0.25 * 8191)
    assert first[1] == bytes((0xD1, 127, 0))
    assert first[3] == bytes((0x91, 60, first[3][2]))
