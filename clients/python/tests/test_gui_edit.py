"""`edit(x)` over the three fundamental structures.

One verb, three editors, and no multitrack anywhere: a curve a script built,
a timeline it filled, a buffer it holds. What is checked is the acceptance the
track was opened with -- two windows over one structure share one stack, an edit
read back is the edit that was drawn, and a window composing two structures
undoes across both in the order the edits were made.
"""

import json

import pytest

from clausters import TempoMap

from clausters.gui import edit
from clausters.gui.editing import (AudioEditor, Editing, Marking, NotesEditor,
                                   PointsEditor)
from clausters.seq import EventSequence, Timeline
from clausters.defs.ugens import Bpf, Env
from clausters.defs import Server
from clausters.base import OscNrtInterface
from clausters.seq.event import Event

SR = 48_000.0
TEMPO = 2.0
BEAT = SR / TEMPO


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.trees: list = []
        self.next = 20_000
        #: What `subscribe` was handed -- an open editor's `apply`.
        self.subscribed: list = []
        self.closed: list = []
        #: What `head_clock` was told: ``(id, which, transport)``.
        self.clocks: list = []

    def alloc_id(self) -> int:
        self.next += 1
        return self.next

    def open(self, tree, id=None):
        self.trees.append(tree)
        return 900 + len(self.trees)

    def define(self, wid, tree):
        self.trees.append(tree)
        return wid

    def ack(self, seq, doc_version=0, reason=None):
        self.acks.append((seq, [], reason))

    def push(self, seq, *corrections, doc_version=0, reason=None):
        self.acks.append((seq, list(corrections), reason))

    def close(self, id):
        self.closed.append(id)

    def set(self, id, **props):
        """A live set, kept per widget so a query answers it -- as the host
        answers what a set wrote."""
        self.__dict__.setdefault("props", {}).setdefault(id, {}).update(props)

    def query(self, id, timeout=1.0):
        from clausters.gui.host import WidgetInfo

        return WidgetInfo("notes", dict(self.__dict__.get("props", {}).get(id, {})))

    def head_clock(self, id, which, transport=0):
        self.clocks.append((id, which, transport))

    def _set_closed_handler(self, id, func):
        self.on_closed = (id, func)

    def poll(self, timeout=0.0):
        return None

    def dispatch(self, *msg):
        pass

    # The event loop's end of the protocol. A double is a host, so it answers
    # the two things an editor asks of one at `open`: what to subscribe to, and
    # a loop -- which a double does not have and does not need, since nothing
    # here is delivered by one.
    def subscribe(self, func):
        self.subscribed.append(func)
        return func

    def unsubscribe(self, func):
        if func in self.subscribed:
            self.subscribed.remove(func)

    looping = False
    loop = None


def picture(tree: dict) -> dict:
    """The view a window edits: the node its `main` names, which stands under
    the toolbar of a window with chrome -- or, in one with none, its first."""
    main = tree.get("main")

    def find(node):
        if node.get("id") == main:
            return node
        for child in node.get("children", ()):
            found = find(child)
            if found is not None:
                return found
        return None

    found = find(tree) if main is not None else None
    return found if found is not None else tree["children"][0]


def a_curve() -> Bpf:
    return Bpf([(0.0, 200.0, "exp"), (2.0, 900.0)])


def a_timeline() -> Timeline:
    return Timeline([(0.0, Event(midinote=60, dur=1.0)),
                     (1.0, Event(midinote=64, dur=1.0))])


def opened(editor):
    host = FakeHost()
    editor.open(host)
    return host, picture(host.trees[0])["id"]


# ---- the verb ----

def test_edit_takes_the_ambient_servers_rate(monkeypatch):
    """With no ``sample_rate``, a curve is laid out at the rate of the server
    a play would resolve -- and at 48 kHz with none."""
    import types

    from clausters.base.main import main

    class Ambient:
        def query_info(self, *_a, **_kw):
            return types.SimpleNamespace(nominal_sample_rate=96_000.0)

    monkeypatch.setattr(main, "server", None)
    monkeypatch.setattr(main, "_ambient_session", lambda: None)
    assert edit(a_curve(), open=False).sample_rate == 48_000.0, "no server anywhere"
    monkeypatch.setattr(main, "server", Ambient())
    assert edit(a_curve(), open=False).sample_rate == 96_000.0
    assert edit(a_curve(), sample_rate=44_100.0, open=False).sample_rate == 44_100.0, "a rate given wins"


def test_the_verb_opens_the_editor_the_structure_asks_for():
    assert isinstance(edit(a_curve(), sample_rate=SR, open=False), PointsEditor)
    assert isinstance(edit(a_timeline(), sample_rate=SR, open=False), NotesEditor)


def test_the_verb_opens_the_window_and_subscribes_the_editor_to_the_host():
    """`edit(x)` is one call: it resolves a host, opens, and plugs the editor
    into what the host delivers -- so ``edit(curve)`` on its own is a complete
    program and nothing writes a drain loop. ``open=False`` is the one case the
    separate `Editor.open` was ever for: a caller composing a window out of
    several editors."""
    host = FakeHost()
    editor = edit(a_curve(), sample_rate=SR, host=host)
    assert editor.id == editor.window and editor.id is not None
    assert editor.closed is False
    assert host.subscribed == [editor.apply]

    editor.close()
    assert editor.closed and editor.id is None
    assert host.subscribed == [], "a closed window stops being delivered to"


def test_open_false_builds_the_editor_without_a_window():
    editor = edit(a_curve(), sample_rate=SR, open=False)
    assert editor.window is None and editor.closed


def test_something_none_of_the_three_reads_says_what_they_are():
    with pytest.raises(TypeError, match="edit` opens a SoundFile"):
        edit(object())


# ---- a curve ----

def test_a_curve_is_drawn_edited_and_read_back_with_no_multitrack():
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    host, wid = opened(editor)

    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       0.0, 300.0, 1, 0.0,
                                       1.0, 500.0, 2, 0.0,
                                       2.0, 100.0, 1, 0.0]) is True
    # Read back through the object the caller already holds: no handing back.
    assert curve.to_points()[0:2] == pytest.approx([0.0, 300.0])
    assert curve.to_points()[4:6] == pytest.approx([1.0, 500.0])
    assert editor.can_undo and editor.undo_label == "draw the curve"

    assert editor.undo() is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0])


def test_marking_is_taken_by_the_editors_that_act_on_what_is_marked():
    """`selected`, `select` and `unselect` are a capability an editor takes:
    the roll and the audio editor have it, and the points editor, where no
    operation acts on a group of points, has none of it -- nor a time range
    of its own."""
    assert issubclass(NotesEditor, Marking)
    assert not {"selected", "select", "unselect"} & set(vars(NotesEditor))
    # the audio editor is the host's application, and its handler marks
    # through the application's transport
    assert {"selected", "select", "unselect"} <= set(vars(AudioEditor))
    assert not issubclass(PointsEditor, Marking)
    editor = edit(a_curve(), sample_rate=SR, open=False)
    for word in ("selected", "select", "unselect", "span"):
        assert not hasattr(editor, word)


def test_a_normalized_envelope_is_kept_inside_its_ranges():
    """Time and value both from 0 to 1: a point a hand reports outside is kept
    inside, and the curve the script holds is written that way."""
    env = Env([0.0, 1.0, 0.0], [0.5, 0.5])
    editor = edit(env, sample_rate=SR, open=False, min=0.0, max=1.0, start=0.0, end=1.0)
    host, wid = opened(editor)
    assert editor.rules == {"values": (0.0, 1.0), "time": (0.0, 1.0)}
    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       0.0, -0.2, 1, 0.0,
                                       0.4, 1.5, 1, 0.0,
                                       1.6, 0.0, 1, 0.0]) is True
    assert env.to_points() == pytest.approx([0.0, 0.0, 1, 0.0,
                                             0.4, 1.0, 1, 0.0,
                                             1.0, 0.0, 1, 0.0])


def test_an_automation_is_kept_in_the_range_of_what_it_automates():
    """With nothing declared, a curve over a parameter is kept in that
    parameter's range -- the one the roll and the multitrack draw it over."""
    from clausters.multitrack import Automation

    cutoff = Automation({"cc": 74}, [(0.0, 10.0), (1.0, 100.0)])
    editor = edit(cutoff, sample_rate=SR, open=False)
    host, wid = opened(editor)
    assert editor.rules == {"values": (0.0, 127.0), "time": None}
    editor.apply("/gui_event", [wid, 1, 0, "points", 0.0, 10.0, 1, 0.0, 1.0, 200.0, 1, 0.0])
    assert cutoff.to_points()[5] == 127.0


def test_a_selected_segment_takes_its_shape_from_the_menu_under_the_curve():
    """A click on a segment selects it; the menu in the row under the curve
    sets its shape, as an edit the curve's history takes back. The script's
    own widgets go in that row, beside the menu."""
    from clausters.gui import button

    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False, extra=[button(name="play")])
    host, wid = opened(editor)
    column = host.trees[0]["children"][1]["children"][0]
    shape = column["children"][0]["id"]
    assert column["children"][1]["name"] == "play"
    assert editor.apply("/gui_event", [wid, 1, 0, "segment", 0]) is False
    assert editor.apply("/gui_event", [shape, 2, 0, 3]) is True
    assert curve.to_points()[2] == 3, "the segment is sine"
    assert editor.undo() is True
    assert curve.to_points()[2] == 2, "and exponential again"


def test_an_edit_made_against_a_picture_an_undo_replaced_is_refused():
    # The staleness floor, on the road an editor actually travels. A host stamps
    # every event with the version it was last told, and it is told only when an
    # acknowledgement reaches it -- a round trip a hand outruns -- so an edit
    # naming an older version is the ordinary case and applies. What does not is
    # an edit made against a picture the data has moved away from by a
    # route the host never saw: here an undo.
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    host, wid = opened(editor)

    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       0.0, 300.0, 1, 0.0,
                                       2.0, 100.0, 1, 0.0]) is True
    #: The version the host has just been told it is drawing, read where it
    #: lives: the version is the editing **context's**, not this view's.
    against = Editing.of(curve).version

    assert editor.undo() is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0])

    # The event the hand had already sent, naming the picture it was made
    # against. Refused rather than applied: an edit-back payload is absolute
    # *and* whole, so applying one made against an older picture would silently
    # drop whatever arrived in between -- here, the undo.
    assert editor.apply("/gui_event", [wid, 2, against, "points",
                                       0.0, 900.0, 1, 0.0,
                                       2.0, 900.0, 1, 0.0]) is False
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0]), "the undo stands"
    assert host.acks[-1][2] == "the data changed since this edit"

    # And a gesture made against the picture that now holds applies.
    assert editor.apply("/gui_event", [wid, 3, Editing.of(curve).version, "points",
                                       0.0, 600.0, 1, 0.0,
                                       2.0, 600.0, 1, 0.0]) is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 600.0])


def test_a_segments_shape_survives_the_round_trip():
    # The crate carries a point's `data` and reads none of it, which is what
    # keeps an undo from putting the curve back straight.
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "points",
                                0.0, 300.0, 5, -4.0,
                                2.0, 900.0, 1, 0.0])
    assert curve.to_points()[2:4] == pytest.approx([5, -4.0]), \
        "the shape the hand drew"
    editor.undo()
    assert curve.to_points()[2:4] == pytest.approx([2, 0.0]), \
        "and the shape it had before (exponential), not a straight line"


def test_a_resend_of_the_curve_is_not_an_edit():
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       *curve.to_points()]) is False
    assert editor.can_undo is False


# ---- an event sequence, and a timeline rendered into one ----

def _roll(tempo=TEMPO):
    """A timeline at ``tempo``, opened: its rendered sequence is what the roll
    edits."""
    timeline = a_timeline()
    timeline.map = TempoMap(tempo)
    editor = edit(timeline, sample_rate=SR, open=False)
    return timeline, editor


def _walk(node):
    yield node
    for child in node.get("children", []) or []:
        yield from _walk(child)


# ---- the acceptance the track was opened with ----

def test_edit_over_a_structure_with_its_editor_open_hands_that_one_back():
    """A second window of one role over one structure would ask for the same
    widgets as the first: `edit` hands back the editor that is open instead,
    and opens nothing. Another role is another window -- a roll in hertz
    beside one in MIDI notes -- and a closed editor is not handed back."""
    host = FakeHost()
    curve = a_curve()
    first = edit(curve, sample_rate=SR, host=host)
    assert edit(curve, sample_rate=SR, host=host, title="again") is first
    assert len(host.trees) == 1, "nothing else was opened"
    # another curve is another window
    assert edit(a_curve(), sample_rate=SR, host=host) is not first
    first.close()
    assert edit(curve, sample_rate=SR, host=host) is not first


def test_two_editors_over_one_structure_share_one_stack():
    curve = a_curve()
    left = edit(curve, sample_rate=SR, open=False)
    right = edit(curve, sample_rate=SR, open=False)
    left_host, wid = opened(left)
    right_host, _ = opened(right)
    right_host.acks.clear()

    left.apply("/gui_event", [wid, 1, 0, "points", 0.0, 400.0, 1, 0.0,
                              2.0, 900.0, 1, 0.0])
    assert right.can_undo, "one pile, whichever window made the edit"
    assert right_host.acks, "and the other window is told what to draw"

    # An undo in *either* updates both, which is the whole claim.
    assert right.undo() is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0])


# ---- a script's change is a turn ----


def test_a_sequence_nobody_asked_a_history_of_records_nothing():
    from clausters.history import ATTR

    seq = EventSequence([(0.0, Event(midinote=60, dur=1.0))])
    seq.events[0].at = 2.0
    seq.events.add(3.0, {"midinote": 62})
    assert getattr(seq, ATTR, None) is None
    assert seq.history.can_undo is False, "asking makes one, empty"


# ---- the picture a view draws is the crate's ----


def test_a_catalogue_view_is_described_by_the_crate_and_not_by_this_client():
    # Which widget draws a take, and the three gestures it offers, were written
    # here, in the web client and in the standalone host. One answer now.
    from clausters import _native

    said = _native.view_props("waveform", {"buffer": 3, "channels": 1,
                                           "ruler": "time",
                                           "sample_rate": SR})
    assert said["type"] == "signal" and said["view"] == "trace"
    assert said["gestures"] == {"drag": "select", "alt": "draw",
                                "ctrl": "sample"}
    assert "id" not in said, "which number a widget gets is the caller's"

    # And the roll's pitch window, which is the other rule that travelled with
    # the picture: one note is its own window, padded.
    roll = _native.view_props("pianoroll", {"notes": [0.0, 1.0, 60.0, 100.0, 0.0]})
    assert roll["axes"]["y"] == {"min": 56.0, "max": 64.0}
    with pytest.raises(ValueError, match='"clip"'):
        _native.view_props("clip", {})
    with pytest.raises(ValueError, match="cannot be drawn"):
        _native.view_props("pianoroll", {"notes": "sixty"})


# ---- the axis a curve is drawn against ----


def test_a_curve_is_drawn_against_its_own_axis_and_not_the_default_one():
    """A `bpf` given no range draws against the unipolar default and fits its
    time to the last point, so a curve of any other range is pinned to the top
    of the field and the edit-back reports the *axis*'s values -- one drag
    destroys the data's range. Both ends come from the curve."""
    curve = a_curve()                       # 200 Hz to 900 Hz, ending at beat 2
    editor = edit(curve, sample_rate=SR, open=False)
    widget = picture(editor.draw())

    band = widget["axes"]["y"]
    assert band["min"] < 200.0 < 900.0 < band["max"], "the curve's own band"
    assert widget["duration"] == pytest.approx(2.0), "and its own span"


def test_the_axis_is_held_rather_than_refitted_under_the_hand():
    """The defect a hand at the window notices first: an axis recomputed on
    every redraw moves the picture while a point is being dragged, so every
    *other* point visibly moves with it. It only ever grows."""
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    first = picture(editor.draw())
    band = first["axes"]["y"]

    # A point dragged down and back up must leave the drawing where it was.
    curve.set_points([0.0, 500.0, 2, 0.0, 2.0, 600.0, 1, 0.0])
    again = picture(editor.draw())
    assert again["axes"]["y"] == band
    assert again["duration"] == first["duration"]

    # And a curve that no longer fits inside it widens the end that stopped
    # holding it, keeping the other.
    curve.set_points([0.0, 200.0, 2, 0.0, 4.0, 9000.0, 1, 0.0])
    wider = picture(editor.draw())["axes"]["y"]
    assert wider["max"] > band["max"] and wider["min"] == band["min"]
    assert picture(editor.draw())["duration"] == pytest.approx(4.0)


# ---- the notes editor plays ----

class _PlayingServer(Server):
    """A server whose transport answers and whose commands are recorded."""

    def __init__(self):
        super().__init__(interface=OscNrtInterface())
        self.latency = 0.1
        self.sent = []
        self.state = {"playing": False}

    def transport_at(self, transport):
        """The transport, as the object a playback plays: its state is this
        double's."""
        from clausters.defs import Transport

        server = self

        class Rolling(Transport):
            def state(self):
                return dict(server.state)

        held = self.__dict__.setdefault("_held", {})
        return held.setdefault(transport, Rolling(self, transport))

    def transport_state(self, timeout=None):
        return dict(self.state)

    def query_info(self, *_a, **_kw):
        class Info:
            nominal_sample_rate = 100.0
        return Info()

    def send_msg(self, addr, *args):
        self.sent.append((addr, args))
        if addr == "/transport_play":
            self.state["playing"] = True

    def request(self, addr, *args, timeout=None, expect=None, match=None):
        self.send_msg(addr, *args)
        return "/done", [addr]

    def lane(self):
        """The note starts of the last `/lane_set`, in samples."""
        import json

        data = [json.loads(str(args[1])) for addr, args in self.sent if addr == "/lane_set"]
        return [note[0] for note in data[-1]["notes"]] if data else None


def test_play_answers_the_transport_the_sequence_plays_on():
    """No window: play(sequence) loads the lane on the server's notes
    transport and answers that transport, whose verbs speak the sequence's
    beats; a change made through the sequence's objects is heard, and the
    pass ends where the contents do."""
    from clausters import play
    from clausters.defs import Transport

    server = _PlayingServer()
    seq = EventSequence([(0.0, Event(midinote=60, dur=1.0)),
                         (2.0, Event(midinote=64, dur=1.0))], tempo_map=TempoMap(TEMPO))
    transport = play(seq, server=server)
    assert isinstance(transport, Transport) and transport.playing
    assert transport is server.transport_at(transport.id), "one transport, one object"
    addrs = [addr for addr, _ in server.sent]
    assert "/lane_new" in addrs and "/transport_end" in addrs, "it ends with its contents"
    assert addrs[-1] == "/transport_play" and server.lane() == [0, 100]

    server.sent.clear()
    seq.events[1].at = 3.0                 # heard from where the position is
    assert [addr for addr, _ in server.sent].count("/lane_set") == 1
    assert server.lane() == [0, 150]

    assert transport.end == "contents"
    transport.end = 4.0                    # an end marker, in its beats
    assert transport.end == 4.0
    server.sent.clear()
    transport.pause()
    transport.locate(1.0)
    transport.loop(0.0, 2.0)
    transport.stop()
    assert server.sent, "each verb is the playback's"
    server.state["playing"] = False        # the pass reached its end
    assert transport.wait(timeout=1.0) is True


def test_two_sequences_play_together_and_a_free_gives_the_transport_back():
    """Each `play(sequence)` takes a transport, so two sound at once; with none
    left the play fails saying so, and a `free` gives one back."""
    from clausters import _native, play

    server = _PlayingServer()
    seqs = [EventSequence([(0.0, Event(midinote=60 + i, dur=1.0))], tempo_map=TempoMap(TEMPO))
            for i in range(16)]
    transports = [play(seq, server=server) for seq in seqs[:15]]
    assert len({t.id for t in transports}) == 15 and min(t.id for t in transports) == 1, \
        "sixteen transports, above the one addressed by number"
    with pytest.raises(RuntimeError, match="--transports"):
        play(seqs[15], server=server)

    server.sent.clear()
    freed = transports[0].free()
    addrs = [addr for addr, _ in server.sent]
    assert "/lane_free" in addrs and "/node_free" in addrs
    assert server.ids.in_use(_native.IdSpaces.TRANSPORTS) == 14
    assert freed.span is None and freed._driver is None, "nothing loaded on it now"
    assert play(seqs[15], server=server).id == freed.id, "the one given back"


def test_a_script_takes_a_transport_of_its_own_and_gives_it_back():
    """`transport_new` takes a free transport, never one that something
    played holds, and `free` gives it back."""
    from clausters import _native, play

    server = _PlayingServer()
    seq = EventSequence([(0.0, Event(midinote=60, dur=1.0))], tempo_map=TempoMap(TEMPO))
    played = play(seq, server=server)
    own = server.transport_new()
    assert own.id not in (0, played.id) and own is server.transport_at(own.id)
    assert server.ids.in_use(_native.IdSpaces.TRANSPORTS) == 2
    own.free()
    assert server.ids.in_use(_native.IdSpaces.TRANSPORTS) == 1
    own.free()                              # twice is nothing
    assert server.transport_at(0).free().id == 0, "nothing to free by number"
    assert server.ids.in_use(_native.IdSpaces.TRANSPORTS) == 1


def test_a_loop_asked_while_stopped_is_kept_for_the_next_play():
    """A short pass ends before a script asks for its loop: the loop is the
    playback's state, kept stopped, and the next play loops the span."""
    server = _PlayingServer()
    seq = EventSequence([(0.0, Event(midinote=60, dur=1.0)),
                         (2.0, Event(midinote=64, dur=1.0))], tempo_map=TempoMap(TEMPO))
    transport = seq.play(server=server)
    server.state["playing"] = False        # the pass reached its end
    transport.loop(0.0, 2.0)
    assert transport.span == (0.0, 2.0) and transport.looping
    server.sent.clear()
    transport.play()
    loops = [args for addr, args in server.sent if addr == "/transport_loop" and len(args) > 1]
    assert [int(a.value if hasattr(a, "value") else a) for a in loops[-1][1:]] == [0, 100], \
        "beats [0, 2) at two beats a second and 100 samples a second"
    transport.unloop()
    assert not transport.looping and transport.span == (0.0, 2.0), "the span stays"


def test_the_window_s_play_is_the_curve_s_holder_s():
    """A curve plays nothing, so the window's play is whoever said it plays it:
    read as available from then on, and called by the verb."""
    played = []
    editor = edit(a_curve(), sample_rate=SR, open=False)
    host, _ = opened(editor)
    window = int(editor.window)
    assert "does not play" in editor.actions["play"]["reason"]
    editor.apply("/gui_event", [window, 1, 0, "play", 0])
    editor.on_play = lambda: played.append(True)
    assert editor.actions["play"]["enabled"] is True
    assert editor.apply("/gui_event", [window, 2, 0, "play", 0]) is False, "no edit"
    assert played == [True]


def test_adopt_draws_the_curve_as_the_script_holds_it():
    """What a script wrote is what `adopt` tells the window: a gesture after it
    is made against that, and takes nothing back to the curve before it."""
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    host, wid = opened(editor)
    curve.set_points([0.0, 0.5, 1, 0.0, 1.0, 0.25, 1, 0.0])
    editor.adopt()
    corrected = [props for _, corrections, _ in host.acks for w, props in corrections if w == wid]
    assert corrected[-1]["points"] == [0.0, 0.5, 1, 0.0, 1.0, 0.25, 1, 0.0]


def test_a_curve_window_has_its_menu_and_says_what_it_does_not_do():
    """A curve's window carries the bar that holds the window's verbs and the
    history, and its readings refuse the rest with why; `chrome=False` draws
    the curve alone. It reads them from the moment it opens."""
    host = FakeHost()
    set_props = []
    host.set = lambda wid, **props: set_props.append((wid, props))
    editor = edit(a_curve(), sample_rate=SR, host=host)
    assert [title["label"] for title in host.trees[-1]["menu"]] == ["File", "Edit", "Help"]
    assert set_props[-1][0] == int(editor.window)
    assert set_props[-1][1]["actions"]["undo"]["enabled"] is False
    assert "clipboard" in editor.actions["paste"]["reason"]
    assert "does not play" in editor.actions["play"]["reason"]
    bare = PointsEditor(a_curve(), sample_rate=SR, chrome=False)
    assert "menu" not in bare.draw()
