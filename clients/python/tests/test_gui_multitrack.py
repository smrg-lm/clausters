"""`clausters.gui.Multitrack` — the piece a `multitrack` widget draws, held here.

No host and no window: a fake widget records what is set on it and hands back
the event a hand's gesture would have sent. What is checked is that the object
is the piece — that a report replaces it whole, and that a script never has to
parse a payload or carry an id.
"""

import pytest

from clausters.gui import Clip, Lane, Multitrack


class FakeWidget:
    """Records `set`s and keeps the handler `attach` registered."""

    def __init__(self):
        self.sets = []
        self.handler = None

    def on_event(self, func):
        self.handler = func
        return self

    def set(self, **props):
        self.sets.append(props)
        return self

    def report(self, tag, *vals):
        """What the widget would have sent after a hand edited the piece."""
        assert self.handler is not None, "nothing subscribed"
        self.handler(tag, *vals)


def piece() -> Multitrack:
    return Multitrack(lanes=[("noise",), ("tone",)],
                      clips=[("a", "noise", 0.0, 500.0),
                             ("b", "tone", 500.0, 500.0)])


def test_the_tuples_a_script_types_become_the_objects_it_reads():
    mt = piece()
    assert [l.name for l in mt.lanes] == ["noise", "tone"]
    assert mt.lanes[0].height == 96.0 and mt.lanes[0].gain == 1.0
    assert mt.clip("b").lane == "tone"
    assert mt.clip("b").end == 1000.0
    assert mt.extent == 1000.0, "the furthest end, not the last onset"
    assert [c.name for c in mt.on("noise")] == ["a"]


def test_one_subscription_carries_the_whole_piece():
    """**The object is the piece.** A gesture reports what the widget now holds,
    so this replaces the lists — there is nothing per clip to register, and a
    script never sees a widget id or parses a payload."""
    mt = piece()
    w = FakeWidget()
    seen = []
    mt.on_change = seen.append
    mt.attach(w)

    # A hand dragged `a` onto the other lane and moved it.
    w.report("clips", "a", "tone", 100.0, 500.0, 0.0, "",
             "b", "tone", 500.0, 500.0, 0.0, "")
    assert seen == ["clips"]
    assert mt.clip("a").lane == "tone"
    assert mt.clip("a").at == 100.0
    assert [c.name for c in mt.on("tone")] == ["a", "b"]

    # And a fader is the other payload, which leaves the clips alone.
    w.report("lanes", "noise", "", 96.0, 1, 0, 0.5, "tone", "", 96.0, 0, 0, 1.0)
    assert seen == ["clips", "lanes"]
    assert mt.lane("noise").mute is True and mt.lane("noise").gain == 0.5
    assert len(mt.clips) == 2, "a lane edit is not a clip edit"


def test_a_change_from_the_script_reaches_the_widget():
    mt = piece()
    w = FakeWidget()
    mt.attach(w)

    mt.place("c", "noise", 1000.0, 200.0)
    assert len(w.sets) == 1 and "clips" in w.sets[-1]
    assert w.sets[-1]["clips"][-1] == ("c", "noise", 1000.0, 200.0, 0.0, "")

    # `place` is one verb: the piece is a statement, so moving is saying where.
    mt.place("c", "tone", 1200.0, 200.0)
    assert len(mt.clips) == 3
    assert mt.clip("c").lane == "tone" and mt.clip("c").at == 1200.0

    mt.mix("noise", gain=0.25, mute=True)
    assert "lanes" in w.sets[-1]
    assert w.sets[-1]["lanes"][0] == ("noise", "", 96.0, True, False, 0.25)


def test_removing_a_lane_keeps_the_clips_that_were_on_it():
    """Losing them silently is the one thing a removal must not do: they name a
    lane that is not there, draw nowhere, and come back to be re-homed."""
    mt = piece()
    mt.remove_lane("noise")
    assert [l.name for l in mt.lanes] == ["tone"]
    assert mt.clip("a") is not None and mt.clip("a").lane == "noise"


def test_a_partial_group_is_dropped_rather_than_half_read():
    mt = piece()
    w = FakeWidget()
    mt.attach(w)
    w.report("clips", "a", "noise", 0.0, 500.0, 0.0, "", "b", "tone", 500.0)
    assert [c.name for c in mt.clips] == ["a"]


def test_the_view_is_built_from_what_the_object_holds():
    mt = Multitrack(lanes=[Lane("one", height=60.0)],
                    clips=[Clip("x", "one", 0.0, 10.0)], snap=4.0)
    spec = dict(mt.view(name="piece", weight=1.0))
    assert spec["type"] == "multitrack"
    assert spec["lanes"] == ["one", "", 60.0, 0, 0, 1.0]
    assert spec["clips"] == ["x", "one", 0.0, 10.0, 0.0, ""]
    assert spec["snap"] == 4.0 and spec["name"] == "piece"


def test_an_unattached_piece_is_still_a_piece():
    """A script may build one before its window exists; nothing is sent, and
    nothing raises."""
    mt = piece()
    mt.place("c", "noise", 0.0, 10.0).mix("noise", gain=0.5)
    assert mt.clip("c") is not None and mt.lane("noise").gain == 0.5


def test_attach_takes_the_window_and_remembers_its_own_name():
    """`view` is a definition and an id names a live widget, so which opened
    window is being watched has to be said. The **name** does not: the object
    built the node, so it knows what it called it."""
    class FakeWindow:
        def __init__(self, widget):
            self._by_name = {"piece": widget}

        def __getitem__(self, name):
            return self._by_name[name]

    mt = piece()
    w = FakeWidget()
    mt.view(name="piece")
    mt.attach(FakeWindow(w))
    w.report("clips", "a", "tone", 7.0, 500.0, 0.0, "")
    assert mt.clip("a").lane == "tone"

    # A view with no name cannot be searched for, and says so.
    nameless = piece()
    nameless.view(weight=1.0)
    with pytest.raises(ValueError, match="no name"):
        nameless.attach(FakeWindow(w))
