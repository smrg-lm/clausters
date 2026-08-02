"""The shared transport (`clausters.gui.transport`) — play/pause/stop/locate and
the view's playhead line.

No host and no server: a fake host records the `/gui_set`s, a fake server answers
the clock query, and the pass is a real `Playhead` driven offline (`clock.render`)
so the end of a pass is reached deterministically. What is checked is the line —
which of the two numbers is written, in which unit — and the state machine around
it, not what the widgets do with it.
"""

import pytest

from clausters.base import TempoClock
from clausters.gui.transport import Transport
from clausters.seq.event import Event as SeqEvent
from clausters.seq.timeline import Playhead, Timeline

SR = 48_000.0
TEMPO = 2.0          # beats per second (120 bpm)
BEAT = SR / TEMPO    # 24000 samples per beat
CLOCK = 1_000_000.0  # the sample-clock value the fake server reports


class FakeHost:
    """Records the `/gui_set`s the transport sends."""

    def __init__(self):
        self.sets = []

    def set(self, id, **props):
        self.sets.append((id, props))

    def last(self, key):
        """The most recent value written for ``key`` (KeyError if never)."""
        return next(props[key] for _id, props in reversed(self.sets)
                    if key in props)


class FakeServer:
    """Answers the anchor's clock query, `latency` seconds ahead of the sound."""

    latency = 0.25

    class interface:
        time_mode = "unix"

    def request(self, addr, expect=None):
        return ("/clock_query.reply", [CLOCK])


class NrtServer(FakeServer):
    class interface:
        time_mode = "score"


class Recorder:
    """A destination that swallows what a pass renders."""

    def play_event(self, event):
        return None


def arp() -> Timeline:
    """Three notes, one per beat: the piece ends at beat 2."""
    return Timeline([(float(i), SeqEvent(midinote=60 + i, dur=1.0))
                     for i in range(3)])


def transport(host=None, clock=None, **kw) -> Transport:
    clock = TempoClock(TEMPO) if clock is None else clock
    dest = Recorder()

    def source(at, **_kw):
        return Playhead(arp(), clock, dest).play(at=at)

    return Transport(FakeHost() if host is None else host, 7, source=source,
                     tempo=TEMPO, sample_rate=SR, **kw)


# ---- the static cursor: the stopped half of the line ----

def test_a_locate_draws_the_cursor_and_turns_the_anchor_off():
    host = FakeHost()
    tp = transport(host)
    tp.locate(3.0)
    assert tp.position == pytest.approx(3.0)
    assert host.sets == [(7, {"playhead_at": -1.0, "playhead": 3 * BEAT})]


def test_the_cursor_is_drawn_in_the_views_own_unit():
    """A page places its cursor in milliseconds, not samples — the whole of what
    a view has to say about its units."""
    host = FakeHost()
    tp = transport(host, to_units=lambda beats: beats * 1000.0 / TEMPO)
    tp.locate(3.0)
    assert host.last("playhead") == pytest.approx(1500.0)


def test_a_locate_never_goes_negative():
    tp = transport()
    tp.locate(-5.0)
    assert tp.position == 0.0


def test_stop_returns_to_the_top_and_pause_keeps_the_position():
    tp = transport()
    tp.locate(5.0)
    tp.pause()                       # nothing playing: the position stands
    assert tp.position == pytest.approx(5.0)
    tp.stop()
    assert tp.position == 0.0


def test_no_host_no_line():
    """A view drawn but not yet opened: the transport still tracks its position."""
    tp = transport()
    tp.host = None
    tp.locate(2.0)
    assert tp.position == pytest.approx(2.0)


# ---- the anchor: the playing half ----

def test_the_anchor_is_the_clock_less_what_has_been_played():
    host = FakeHost()
    tp = transport(host)
    assert tp.anchor(FakeServer(), at=2.0)
    # Items sound `latency` ahead, so beat 0 sits that much further along.
    expected = CLOCK + FakeServer.latency * SR - 2 * BEAT
    assert host.last("playhead_at") == pytest.approx(expected)


def test_an_nrt_destination_has_nothing_to_anchor_to():
    host = FakeHost()
    assert not transport(host).anchor(NrtServer())
    assert host.sets == [], "and it says so rather than drawing a still line"


def test_a_destination_that_cannot_be_asked_answers_false():
    assert not transport().anchor(object())


def test_playing_takes_the_line_over_from_the_cursor():
    host = FakeHost()
    tp = transport(host)
    tp.play(FakeServer(), at=1.0)
    assert tp.playing
    assert tp.position == pytest.approx(1.0)
    # The cursor is cleared first, then the clock anchor is set.
    assert host.sets[-2][1] == {"playhead_at": -1.0, "playhead": -1.0}
    assert host.sets[-1][1]["playhead_at"] == pytest.approx(
        CLOCK + FakeServer.latency * SR - BEAT)


def test_pause_holds_the_cursor_where_the_music_stopped():
    host = FakeHost()
    tp = transport(host)
    tp.play(FakeServer(), at=1.0)
    tp.pause()
    assert not tp.playing
    assert host.last("playhead_at") == -1.0
    assert host.last("playhead") == pytest.approx(BEAT)


def test_a_seek_while_playing_starts_a_fresh_pass():
    """Which is what makes one `locate` serve as rewind, too."""
    tp = transport()
    first = tp.play(FakeServer(), at=0.0)
    tp.locate(2.0)
    assert tp.playing and tp.playhead is not first
    assert tp.position == pytest.approx(2.0)


def test_a_bare_play_resumes_from_where_it_was_left():
    tp = transport()
    tp.locate(1.0)
    tp.play(FakeServer())
    assert tp.position == pytest.approx(1.0)


# ---- the end of a pass, parked without timing it ----

def test_the_end_of_a_pass_parks_the_cursor_at_the_extent():
    host = FakeHost()
    clock = TempoClock(TEMPO)
    tp = transport(host, clock=clock, extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    clock.render()                          # the pass runs out

    assert tp.update(), "the piece just ended"
    assert not tp.playing
    assert tp.position == pytest.approx(3.0), "parked at the piece's end"
    assert host.last("playhead") == pytest.approx(3 * BEAT)
    assert host.last("playhead_at") == -1.0


def test_the_end_is_reported_once():
    clock = TempoClock(TEMPO)
    tp = transport(clock=clock, extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    clock.render()
    assert tp.update()
    assert not tp.update(), "a parked cursor is not re-sent every pass of the loop"


def test_without_an_extent_it_parks_on_the_last_item():
    tp = transport(clock=(clock := TempoClock(TEMPO)))
    tp.play(FakeServer(), at=0.0)
    clock.render()
    tp.update()
    assert tp.position == pytest.approx(2.0), "the last note's own onset"


def test_a_locate_after_the_end_stands():
    """Seeking away from the end must not be undone by the next `update`."""
    tp = transport(clock=(clock := TempoClock(TEMPO)), extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    clock.render()
    tp.update()
    tp.locate(1.0)
    assert not tp.update()
    assert tp.position == pytest.approx(1.0)


def test_a_pass_stopped_by_hand_did_not_end():
    tp = transport(extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    tp.pause()
    assert not tp.update()


# ---- the widgets the line goes to ----

def test_the_ids_are_read_on_each_use():
    """A view that redraws has new widget ids, and the line must find them."""
    host = FakeHost()
    lanes = [10, 11]
    tp = transport(host)
    tp.ids = lambda: lanes
    tp.locate(1.0)
    assert [wid for wid, _ in host.sets] == [10, 11]

    lanes = [20]                            # redrawn: a new lane
    tp.locate(2.0)
    assert host.sets[-1][0] == 20


# ---- play versus resume: MIDI's start versus continue ----

class FakeGovernedServer:
    """A server whose transport governs a subtree: it records the calls rather
    than freezing anything."""

    def __init__(self):
        self.calls = []

    def transport_stop(self):
        self.calls.append("stop")

    def transport_play(self, position=None):
        self.calls.append("play")


def test_resume_does_not_re_render():
    """Play restarts the material; resume continues it.

    Governed, re-rendering on a resume would restart the very nodes the server
    froze so they could carry on -- which is the whole point of the freeze.
    """
    calls = []
    clock = TempoClock(TEMPO)
    dest = Recorder()

    def source(at, **_kw):
        calls.append(at)
        return Playhead(arp(), clock, dest).play(at=at)

    server = FakeGovernedServer()
    tp = Transport(FakeHost(), 7, source=source, tempo=TEMPO, sample_rate=SR,
                   clock=clock, governed=True)
    tp.server = server

    tp.play(at=0.0)
    assert calls == [0.0]
    tp.pause()
    tp.resume()
    assert calls == [0.0], "resume must not call source again"
    assert server.calls == ["stop", "play"]


def test_play_still_re_renders():
    calls = []
    clock = TempoClock(TEMPO)
    dest = Recorder()

    def source(at, **_kw):
        calls.append(at)
        return Playhead(arp(), clock, dest).play(at=at)

    tp = Transport(FakeHost(), 7, source=source, tempo=TEMPO, sample_rate=SR)
    tp.play(at=0.0)
    tp.pause()
    tp.play()
    assert len(calls) == 2, "play reads the material as it now stands"


def test_a_governed_pause_starves_the_playhead_instead_of_stopping_it():
    clock = TempoClock(TEMPO)
    dest = Recorder()
    heads = []

    def source(at, **_kw):
        ph = Playhead(arp(), clock, dest).play(at=at)
        heads.append(ph)
        return ph

    tp = Transport(FakeHost(), 7, source=source, tempo=TEMPO, sample_rate=SR,
                   clock=clock, governed=True)
    tp.server = FakeGovernedServer()
    tp.play(at=0.0)
    tp.pause()
    assert heads[0].playing, "the playhead is not stopped, it runs out of time"
    assert clock.frozen


def test_an_ungoverned_pause_still_stops_the_playhead():
    clock = TempoClock(TEMPO)
    dest = Recorder()
    heads = []

    def source(at, **_kw):
        ph = Playhead(arp(), clock, dest).play(at=at)
        heads.append(ph)
        return ph

    tp = Transport(FakeHost(), 7, source=source, tempo=TEMPO, sample_rate=SR)
    tp.play(at=0.0)
    tp.pause()
    assert not heads[0].playing
