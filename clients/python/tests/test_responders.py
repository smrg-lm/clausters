"""C13 client: OscFunc / MidiFunc dispatch.

The OSC side runs end to end over a loopback UDP socket — a real `OscReceiver`
binds a port, an `OscFunc` registers, and a datagram (message and bundle) is
sent in; the callback records what it saw. The MIDI side tests `parse_midi` and
the `MidiFunc` matching against injected messages (a real ALSA virtual port
can't be driven without hardware; that path is the manual E2E in
`clients/python/examples/io/midi_responder.py`). No Clausters server is needed for either.
"""

import socket
import time

import pytest

from clausters.base import OscReceiver, parse_midi
from clausters.base import _osclib as osc
from clausters.base._midiinterface import MidiReceiver
from clausters.responders import MidiFunc, OscFunc, midifunc, oscfunc


# ---- helpers ----


def _send(receiver, packet):
    """Send a raw OSC packet to a receiver's bound port over loopback."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.sendto(packet, ("127.0.0.1", receiver.port))
    sock.close()


def _wait(predicate, timeout=2.0):
    """Spin until ``predicate()`` is true (the receiver thread is async)."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return False


# ---- OSC (live over loopback) ----


def test_oscfunc_matches_address_and_args():
    recv = OscReceiver().start()
    try:
        got = []
        OscFunc(lambda msg, t, src: got.append(msg), "/play", recv=recv)
        # A non-matching address is ignored.
        _send(recv, osc.message("/other", 1))
        _send(recv, osc.message("/play", 440, 0.5))
        assert _wait(lambda: got)
        time.sleep(0.05)
        assert got == [["/play", 440, pytest.approx(0.5)]]
    finally:
        recv.stop()


def test_oscfunc_arg_template_filters():
    recv = OscReceiver().start()
    try:
        got = []
        # Respond only to /ctl messages whose first arg is 7; second arg any.
        OscFunc(lambda msg, t, src: got.append(msg[2]), "/ctl",
                arg_template=[7, None], recv=recv)
        _send(recv, osc.message("/ctl", 3, 100))   # filtered out
        _send(recv, osc.message("/ctl", 7, 200))   # matches
        assert _wait(lambda: got)
        time.sleep(0.05)
        assert got == [200]
    finally:
        recv.stop()


def test_oscfunc_bundle_is_unwrapped_with_time():
    recv = OscReceiver().start()
    try:
        got = []
        OscFunc(lambda msg, t, src: got.append((msg, t)), "/at", recv=recv)
        when = time.time() + 100.0
        _send(recv, osc.bundle_at(when, osc.message("/at", 1)))
        assert _wait(lambda: got)
        msg, t = got[0]
        assert msg == ["/at", 1]
        assert t == pytest.approx(when, abs=1e-3)
    finally:
        recv.stop()


def test_oscfunc_one_shot_frees_after_first():
    recv = OscReceiver().start()
    try:
        got = []
        OscFunc(lambda msg, t, src: got.append(msg), "/ping", recv=recv).one_shot()
        _send(recv, osc.message("/ping"))
        _send(recv, osc.message("/ping"))
        assert _wait(lambda: got)
        time.sleep(0.05)
        assert len(got) == 1
    finally:
        recv.stop()


def test_oscfunc_disable_and_enable():
    recv = OscReceiver().start()
    try:
        got = []
        f = OscFunc(lambda msg, t, src: got.append(1), "/p", recv=recv)
        f.disable()
        _send(recv, osc.message("/p"))
        time.sleep(0.1)
        assert got == []
        f.enable()
        _send(recv, osc.message("/p"))
        assert _wait(lambda: got)
    finally:
        recv.stop()


def test_oscfunc_decorator():
    recv = OscReceiver().start()
    try:
        got = []

        @oscfunc("/deco", recv=recv)
        def resp(msg, t, src):
            got.append(msg)

        assert isinstance(resp, OscFunc)
        _send(recv, osc.message("/deco", 9))
        assert _wait(lambda: got)
        assert got == [["/deco", 9]]
    finally:
        recv.stop()


# ---- MIDI parsing ----


def test_parse_midi_channel_voice():
    assert parse_midi(b"\x90\x3c\x64") == {
        "type": "note_on", "channel": 0, "note": 60, "velocity": 100
    }
    assert parse_midi(b"\x82\x3c\x00") == {
        "type": "note_off", "channel": 2, "note": 60, "velocity": 0
    }
    assert parse_midi(b"\xb0\x07\x7f") == {
        "type": "control_change", "channel": 0, "control": 7, "value": 127
    }
    assert parse_midi(b"\xc1\x05") == {
        "type": "program_change", "channel": 1, "program": 5
    }


def test_parse_midi_pitchwheel_is_14bit():
    # LSB=0x00, MSB=0x40 -> centre 8192.
    assert parse_midi(b"\xe0\x00\x40") == {
        "type": "pitchwheel", "channel": 0, "pitch": 8192
    }


def test_parse_midi_rejects_non_channel_voice():
    assert parse_midi(b"\xf8") is None       # timing clock
    assert parse_midi(b"") is None
    assert parse_midi(b"\x3c") is None        # a data byte, no status


# ---- MidiFunc matching (injected, no real port) ----


class _FakeMidiReceiver(MidiReceiver):
    """A `MidiReceiver` whose port is never opened; ``feed`` injects a raw
    message straight into the demux path, so the matching logic is testable
    without ALSA."""

    def __init__(self):
        super().__init__()

    def feed(self, raw):
        msg = parse_midi(raw)
        if msg is not None:
            self._dispatch(msg)


def test_midifunc_matches_type_and_channel():
    recv = _FakeMidiReceiver()
    got = []
    MidiFunc(lambda m, src: got.append(m["note"]), "note_on", chan=1, recv=recv)
    recv.feed(b"\x80\x3c\x00")   # note_off: wrong type
    recv.feed(b"\x90\x3c\x64")   # note_on ch0: wrong channel
    recv.feed(b"\x91\x40\x64")   # note_on ch1: matches
    assert got == [64]


def test_midifunc_list_of_types_and_template():
    recv = _FakeMidiReceiver()
    got = []
    MidiFunc(lambda m, src: got.append(m["type"]), ["note_on", "note_off"],
             arg_template={"note": lambda n: n >= 60}, recv=recv)
    recv.feed(b"\x90\x30\x64")   # note 48: template rejects
    recv.feed(b"\x90\x3c\x64")   # note 60 on: matches
    recv.feed(b"\x80\x3c\x00")   # note 60 off: matches
    assert got == ["note_on", "note_off"]


def test_midifunc_one_shot_and_decorator():
    recv = _FakeMidiReceiver()
    got = []

    @midifunc("note_on", recv=recv)
    def resp(m, src):
        got.append(m["note"])

    resp.one_shot()
    recv.feed(b"\x90\x3c\x64")
    recv.feed(b"\x90\x3e\x64")
    assert got == [60]
