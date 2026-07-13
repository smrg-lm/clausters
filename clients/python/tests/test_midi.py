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
    clock = TempoClock(tempo=1.0)
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
    clock = TempoClock(tempo=1.0)
    Pbind(instrument="default", freq=Pseq([440.0]), dur=1.0, amp=1.0).play(clock, midi)
    clock.render()
    on = _note_ons(midi.score.sorted())[0]
    assert on[1][1] == 69  # 440 Hz -> MIDI note 69
    assert on[1][2] == 127  # amp 1.0 -> max velocity


def _midi_or_skip():
    try:
        from clausters import _midi

        _midi.lib()
    except OSError as e:
        pytest.skip(f"clausters-midi not built: {e}")


def _rendered_midi(**pbind):
    midi = MidiServer()
    clock = TempoClock(tempo=1.0)
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
        clock = TempoClock(tempo=4.0)
        Pbind(instrument="default", midinote=Pseq([60, 64, 67]), dur=0.25, amp=0.7).play(
            clock, midi
        )
        clock.render()  # drives the routine: note-ons now, note-offs scheduled
    finally:
        iface.close()
