"""The shared playhead sync (`clausters.gui.playhead_sync`) — play/pause/stop/locate and
the view's playhead line.

No host and no server: a fake host records the `/gui_set`s, a fake server answers
the clock query, and the pass is a stub whose end is reached by hand (what a
real timeline does with its own clock is `test_timeline_play.py`'s). What is checked is the line —
which of the two numbers is written, in which unit — and the state machine around
it, not what the widgets do with it.
"""

import pytest

from clausters.base import TempoClock
from clausters.gui.playhead_sync import PlayheadSync
from clausters.seq.event import Event as SeqEvent
from clausters.seq.timeline import Timeline

SR = 48_000.0
TEMPO = 2.0          # beats per second (120 bpm)
BEAT = SR / TEMPO    # 24000 samples per beat
CLOCK = 1_000_000.0  # the sample-clock value the fake server reports
#: What the passes play, as far as the line is concerned: its map is the tempo.
TIMELINE = Timeline(tempo=TEMPO)


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


class Pass:
    """What a `source` hands back: the timeline it is playing, as `PlayheadSync`
    reads it — its map, its position, whether it is playing, and whether it ran
    out. A stub, so a test decides when the plan runs out; what a real timeline
    does with its own clock is `test_timeline_play.py`'s."""

    #: The beats its items sit on — the last one is where a drained plan stops.
    items = (0.0, 1.0, 2.0)

    def __init__(self, clock, at=0.0):
        self.clock = clock
        self.map = TIMELINE.map
        self._at = float(at)
        self.playing = True
        self.finished = False
        self.scanned_at = None

    def position(self):
        return self._at

    def pause(self):
        self.playing = False

    def stop(self):
        self.playing = False

    def locate(self, beat):
        self._at = float(beat)
        self.finished = False

    def drain(self):
        """The plan runs out on its **last item**, which is where a real one
        leaves its position while that item is still sounding."""
        self._at = float(self.items[-1])
        self.playing = False
        self.finished = True
        self.scanned_at = self.clock.beats()


def transport(host=None, clock=None, **kw) -> PlayheadSync:
    clock = TempoClock(TEMPO) if clock is None else clock

    def source(at, **_kw):
        return Pass(clock, at)

    return PlayheadSync(FakeHost() if host is None else host, 7, source=source,
                     structure=TIMELINE, sample_rate=SR, **kw)


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
    tp.playhead.drain()                     # the plan runs out

    assert tp.update(), "the pass just ended"
    assert not tp.playing
    assert tp.position == pytest.approx(3.0), "parked at the pass's end"
    assert host.last("playhead") == pytest.approx(3 * BEAT)
    assert host.last("playhead_at") == -1.0


class ClockedHost(FakeHost):
    """A host with an application clock, recording what is scheduled on it."""

    class Clock:
        def __init__(self):
            self.scheduled = []

        def sched(self, delay, func):
            self.scheduled.append((delay, func))
            return func

    def __init__(self):
        super().__init__()
        self.clock = self.Clock()


def test_a_play_puts_the_end_of_the_pass_on_the_application_clock():
    """`update` used to be "call it once per pass of the script's loop", which is
    a hand-written pump by another name. A play schedules it on the host's own
    clock instead, and it stops asking when the pass stops sounding."""
    host = ClockedHost()
    clock = TempoClock(TEMPO)
    tp = transport(host, clock=clock, extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)

    assert len(host.clock.scheduled) == 1, "one tick, scheduled by the play"
    delay, tick = host.clock.scheduled[0]
    assert delay > 0.0
    assert tick() == delay, "a number keeps it going: the loop reschedules by it"

    tp.playhead.drain()                     # the plan runs out
    assert tick() == delay, "the drained scan is what it is there to notice"
    assert tp.position == pytest.approx(3.0), "so the cursor parks at the end"
    assert tick() is None, "and having parked, it stops asking"

    tp.play(FakeServer(), at=0.0)
    assert len(host.clock.scheduled) == 2, "the next play starts it again"


def test_a_transport_with_no_host_clock_keeps_update_manual():
    """A view built before it is opened has no clock to schedule on, and `update`
    is the plain call it always was."""
    tp = transport(FakeHost(), clock=(clock := TempoClock(TEMPO)), extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    tp.playhead.drain()
    assert tp.update()


def test_the_end_is_reported_once():
    clock = TempoClock(TEMPO)
    tp = transport(clock=clock, extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    tp.playhead.drain()
    assert tp.update()
    assert not tp.update(), "a parked cursor is not re-sent every pass of the loop"


def test_without_an_extent_it_parks_on_the_last_item():
    tp = transport(clock=(clock := TempoClock(TEMPO)))
    tp.play(FakeServer(), at=0.0)
    tp.playhead.drain()
    tp.update()
    assert tp.position == pytest.approx(2.0), "the last note's own onset"


class RollingClock(TempoClock):
    """A clock whose beat is set by hand instead of by a thread — a *rolling*
    clock (its beat is the wall's, so a transport may sweep the last item's
    tail over it) that a test can move deterministically."""

    def __init__(self, tempo):
        super().__init__(tempo)
        self._beat = 0.0

    @property
    def rolling(self) -> bool:
        return True     # a driven clock, whatever `render` left the mode on

    def beats(self) -> float:
        return self._beat

    def advance(self, beats: float):
        self._beat += beats


def test_the_last_item_keeps_the_line_until_the_piece_actually_ends():
    """A scan runs out when it renders its **last item**, and the last clip is
    still sounding then. Parking the cursor there jumps the line to the end
    while the sound goes on — so the drained scan starts a *tail* the line
    sweeps, and only its end parks the cursor."""
    host = FakeHost()
    clock = RollingClock(TEMPO)
    tp = transport(host, clock=clock, extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    tp.playhead.drain()                     # the plan runs out on its last item

    anchored = host.last("playhead_at")
    assert not tp.update(), "the last item is still sounding"
    assert tp.position == pytest.approx(2.0), "the last item's onset"
    assert host.last("playhead_at") == anchored, "the line is left sweeping"

    clock.advance(0.5)                      # half a beat into that last item
    assert not tp.update()
    assert tp.position == pytest.approx(2.5)
    assert tp.playing, "the last note is still sounding, so the button says pause"

    clock.advance(0.6)                      # past the pass's end
    assert tp.update(), "the pass ended"
    assert not tp.playing
    assert tp.position == pytest.approx(3.0)
    assert host.last("playhead") == pytest.approx(3 * BEAT)
    assert host.last("playhead_at") == -1.0


def test_a_pause_inside_the_tail_holds_where_the_music_is():
    clock = RollingClock(TEMPO)
    tp = transport(clock=clock, extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    tp.playhead.drain()
    clock.advance(0.5)
    tp.update()
    tp.pause()
    assert tp.at == pytest.approx(2.5), "not the beat the pass started from"


def test_a_locate_after_the_end_stands():
    """Seeking away from the end must not be undone by the next `update`."""
    tp = transport(clock=(clock := TempoClock(TEMPO)), extent=lambda: 3.0)
    tp.play(FakeServer(), at=0.0)
    tp.playhead.drain()
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
    """Play restarts the take; resume continues it.

    Governed, re-rendering on a resume would restart the very nodes the server
    froze so they could carry on -- which is the whole point of the freeze.
    """
    calls = []
    clock = TempoClock(TEMPO)

    def source(at, **_kw):
        calls.append(at)
        return Pass(clock, at)

    server = FakeGovernedServer()
    tp = PlayheadSync(FakeHost(), 7, source=source, structure=TIMELINE, sample_rate=SR,
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

    def source(at, **_kw):
        calls.append(at)
        return Pass(clock, at)

    tp = PlayheadSync(FakeHost(), 7, source=source, structure=TIMELINE, sample_rate=SR)
    tp.play(at=0.0)
    tp.pause()
    tp.play()
    assert len(calls) == 2, "play reads the take as it now stands"


def test_a_governed_pause_starves_the_playhead_instead_of_stopping_it():
    clock = TempoClock(TEMPO)
    heads = []

    def source(at, **_kw):
        ph = Pass(clock, at)
        heads.append(ph)
        return ph

    tp = PlayheadSync(FakeHost(), 7, source=source, structure=TIMELINE, sample_rate=SR,
                   clock=clock, governed=True)
    tp.server = FakeGovernedServer()
    tp.play(at=0.0)
    tp.pause()
    assert heads[0].playing, "the playhead is not stopped, it runs out of time"
    assert clock.frozen


def test_an_ungoverned_pause_still_stops_the_playhead():
    clock = TempoClock(TEMPO)
    heads = []

    def source(at, **_kw):
        ph = Pass(clock, at)
        heads.append(ph)
        return ph

    tp = PlayheadSync(FakeHost(), 7, source=source, structure=TIMELINE, sample_rate=SR)
    tp.play(at=0.0)
    tp.pause()
    assert not heads[0].playing


# ---- the transport: the server owns the position, and the host reads it ----

class TransportServer(FakeServer):
    """A server that answers for its transport: it records the commands and
    answers where it is."""

    def __init__(self):
        self.calls = []
        self.state = {"playing": False, "position_sample": 0, "loop": None}

    def transport_play(self, position=None, timeout=None):
        self.calls.append(("play", position))
        self.state["playing"] = True

    def transport_stop(self, timeout=None):
        self.calls.append(("stop",))
        self.state["playing"] = False

    def transport_locate_sample(self, sample, timeout=None):
        self.calls.append(("locate", int(sample)))
        self.state["position_sample"] = int(sample)

    def transport_loop(self, span=None, timeout=None):
        self.calls.append(("loop", span))
        self.state["loop"] = span

    def transport_state(self, timeout=None):
        return dict(self.state)


class HeadClockHost(FakeHost):
    """A host that also records `head_clock`."""

    def __init__(self):
        super().__init__()
        self.head = None

    def head_clock(self, which):
        self.head = which


def transport_sync(host=None, server=None):
    host = HeadClockHost() if host is None else host
    tp = PlayheadSync(host, 7, head_clock="transport", structure=TIMELINE, sample_rate=SR)
    tp.server = TransportServer() if server is None else server
    return tp


def test_a_transport_sync_tells_the_host_which_counter_to_draw():
    """The two halves of one decision, so they cannot disagree: the client stops
    computing the line and the host starts reading the transport's position."""
    host = HeadClockHost()
    tp = transport_sync(host)
    assert host.head == "transport"
    # And the anchor is 0, because the counter already *is* the transport's time.
    tp.play()
    assert host.last("playhead_at") == 0.0


def test_a_transport_syncs_verbs_are_the_servers():
    tp = transport_sync()
    tp.play()
    tp.pause()
    tp.locate(3.0)
    tp.stop()
    assert tp.server.calls == [
        ("play", None),
        ("stop",),
        ("locate", int(3 * BEAT)),
        ("stop",),
        ("locate", 0),
    ]


def test_a_transport_sync_reads_where_it_is_instead_of_keeping_it():
    """The whole point: the position is the engine's, so a locate nobody here
    sent -- a loop's wrap, another client's seek -- is still where it says."""
    tp = transport_sync()
    tp.server.state["position_sample"] = int(5 * BEAT)
    tp.server.state["playing"] = True
    assert tp.position == 0.0, "nothing was asked yet, so nothing is known yet"
    tp.refresh()
    assert tp.position == pytest.approx(5.0)
    assert tp.playing


def test_a_locate_while_the_piece_plays_does_not_re_cue_anything():
    """A device-clock transport throws the pass away and starts another; the
    transport's seeks in the engine, so the sound carries on from there."""
    tp = transport_sync()
    tp.play()
    tp.server.calls.clear()
    tp.locate(4.0)
    assert tp.server.calls == [("locate", int(4 * BEAT))], \
        "one seek, and no second play"


def test_a_transport_sync_loops_in_the_engine():
    tp = transport_sync()
    tp.loop(1.0, 3.0)
    assert tp.server.calls[-1] == ("loop", (int(1 * BEAT), int(3 * BEAT)))
    tp.loop(None)
    assert tp.server.calls[-1] == ("loop", None)


def test_a_piece_still_cues_a_pass_of_voices_and_only_on_a_locate():
    """The two halves meet in `play`: what follows the transport by itself needs
    no pass, and what fires voices does — so a source is still called, and a
    locate cues it again while nothing re-cues on an edit."""
    cued = []

    def source(at, **_kw):
        cued.append(at)
        return None

    host = HeadClockHost()
    tp = PlayheadSync(host, 7, head_clock="transport", source=source,
                   structure=TIMELINE, sample_rate=SR)
    tp.server = TransportServer()
    tp.play()
    assert cued == [0.0]
    tp.locate(2.0)
    assert cued == [0.0, 2.0], "the one re-cue the transport keeps"
    tp.pause()
    tp.locate(4.0)
    assert cued == [0.0, 2.0], "stopped, there is no pass to cue"


def test_the_map_is_asked_of_what_plays_and_never_kept():
    """No tempo is held here: beats cross through the pass's own map (a
    timeline holds one), else the structure's, read on each use."""
    from clausters import TempoMap

    class Pass:
        map = TempoMap(4.0)
        playing = True

        def position(self):
            return 0.0

        def pause(self):
            self.playing = False

    timeline = Timeline(tempo=TEMPO)
    tp = PlayheadSync(FakeHost(), 7, source=lambda at, **_: Pass(),
                      structure=timeline, sample_rate=SR)
    assert tp.beats_to_samples(1.0) == pytest.approx(BEAT)
    timeline.map.push(0.0, 1.0)                 # edited on the structure: followed
    assert tp.beats_to_samples(1.0) == pytest.approx(SR)
    tp.play(FakeServer(), at=0.0)
    assert tp.beats_to_samples(1.0) == pytest.approx(SR / 4.0)


def test_with_no_map_to_ask_positions_are_seconds():
    """What holds no tempo map -- a multitrack, placed in physical time --
    plays in seconds, which cross to samples as they are."""
    tp = PlayheadSync(FakeHost(), 7, sample_rate=SR)
    assert tp.beats_to_samples(1.0) == pytest.approx(SR)
    assert tp.samples_to_beats(SR / 2.0) == pytest.approx(0.5)
