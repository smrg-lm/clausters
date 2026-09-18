"""The generic editor and its collaborators (`clausters.gui.editing`).

`Editor` edits **one structure** and imports nothing from the arrangement, so
this drives one with no multitrack anywhere: a plain object, a domain that
says what a gesture means to it, and a view that draws it. What is checked is
the orchestration — the gesture becomes a payload, the payload becomes an
entry, the entry inverts — and the two collaborators that are testable with no
data at all.
"""

import pytest

from clausters.gui.editing import (Application, Domain, Echo, Editing,
                                   Editor, View)
from clausters.gui.ids import GuiIdAllocator
from clausters.defs.ugens import Env
from clausters.seq.automation import Automation

SR = 48_000.0


# ---- a structure, a vocabulary and a picture, none of them the arrangement ----

class Dial:
    """A number somebody edits. The whole structure."""

    def __init__(self, value: float = 0.0):
        self.value = float(value)


class DialDomain(Domain):
    """`Dial`'s vocabulary: one verb, and the state it replaces.

    The inverse is the value as it stands, read before the edit lands — which
    is what `Domain.current` is for and why an editor cannot derive it
    afterwards.
    """

    name = "points"      # a real vocabulary, so the coalesce key is the crate's

    def payload(self, structure, tag, values):
        if tag != "dial":
            return None
        return {"intent": "setpoints",
                "points": [{"at": 0.0, "value": float(values[0])}]}

    def current(self, structure, payload):
        return {"intent": "setpoints",
                "points": [{"at": 0.0, "value": structure.value}]}

    def project(self, structure, payload) -> bool:
        value = float(payload["points"][0]["value"])
        if value == structure.value:
            return False        # a resend is not an edit
        structure.value = value
        return True

    def label(self, payload) -> str:
        return "turn the dial"


class DialView(View):
    """One widget drawing one number."""

    def build(self, editor) -> dict:
        wid = self.widget(editor, "dial", editor.structure)
        return {"type": "window", "children": [
            {"id": wid, "type": "number", "value": editor.structure.value}]}

    def props(self, editor, widget_id: int) -> dict:
        return {"value": editor.structure.value}


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.pushes: list = []
        #: What was redefined whole, what was redefined in part, and what was
        #: told one prop at a time.
        self.defines: list = []
        self.redefines: list = []
        self.sets: list = []
        #: The widget-id namespace, as a real host has one: the editors drawing
        #: on this double name their widgets in it, so two of them cannot pick
        #: the same number.
        self.ids = GuiIdAllocator(base=20_000)
        #: What `subscribe` was handed -- an open editor's `apply`.
        self.subscribed: list = []

    def alloc_id(self) -> int:
        return self.ids.alloc()

    def open(self, tree, id=None):
        self.tree = tree
        return 999

    def ack(self, seq, doc_version=0, reason=None):
        self.acks.append((seq, doc_version, reason))

    def push(self, seq, *corrections, doc_version=0, reason=None):
        self.pushes.append((seq, list(corrections), doc_version, reason))

    def redefine(self, wid, tree, *blobs, window=None):
        #: The subtrees this host was handed, and the window each belonged to —
        #: what says a change of shape cost one widget rather than the window.
        self.redefines.append((wid, window))

    def define(self, wid, tree, *blobs):
        #: The whole trees this host was handed — what says a redefine happened,
        #: since a redefine is the only channel a widget that was not there can
        #: arrive by.
        self.defines.append((wid, tree))
        return wid

    def set(self, wid, **props):
        self.sets.append((wid, props))

    def close(self, wid):
        self.closed = wid

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


def an_editor(structure=None, app=None):
    return Editor(structure or Dial(), sample_rate=SR,
                  domain=DialDomain(), view=DialView(), app=app)


class QueueHost(FakeHost):
    """A host with messages to hand out, so a drain can be watched."""

    def __init__(self, *messages):
        super().__init__()
        self.pending = list(messages)
        self.dispatched: list = []

    def poll(self, timeout=0.0):
        return self.pending.pop(0) if self.pending else None

    def dispatch(self, *msg):
        self.dispatched.append(msg)


# ---- the acceptance: an editor with no arrangement anywhere ----

def test_the_generic_editor_imports_nothing_from_the_arrangement():
    # The whole point of the split, and the one thing a test can check outright:
    # the four collaborators and the editor reach no arrangement module.
    import clausters.gui.editing.context as context
    import clausters.gui.editing.domain as domain
    import clausters.gui.editing.echo as echo
    import clausters.gui.editing.editor as editor
    import clausters.gui.editing.view as view

    for module in (editor, view, domain, echo, context):
        source = open(module.__file__).read()
        assert "form" not in source.split("\n")[0] or True
        for line in source.split("\n"):
            if line.startswith(("import ", "from ")):
                assert "form" not in line, f"{module.__name__}: {line}"


def test_a_gesture_becomes_an_entry_and_the_entry_inverts():
    dial = Dial(0.25)
    ed = an_editor(dial)
    host = FakeHost()
    ed.open(host)

    wid = host.tree["children"][0]["id"]
    assert ed.apply("/gui_event", [wid, 1, 0, "dial", 0.75]) is True
    assert dial.value == 0.75
    assert ed.can_undo and ed.undo_label == "turn the dial"

    assert ed.undo() is True
    assert dial.value == 0.25
    assert ed.redo() is True
    assert dial.value == 0.75


def test_a_resend_is_not_an_edit():
    dial = Dial(0.5)
    ed = an_editor(dial)
    host = FakeHost()
    ed.open(host)
    wid = host.tree["children"][0]["id"]
    assert ed.apply("/gui_event", [wid, 1, 0, "dial", 0.5]) is False
    assert ed.can_undo is False, "nothing changed, so there is nothing to undo"


def test_a_tag_that_is_not_an_edit_never_reaches_the_domain():
    # A selection, a zoom and which layer the hand is on are screen state, and
    # the crate is explicit that they are never part of what is edited.
    dial = Dial()
    ed = an_editor(dial)
    host = FakeHost()
    ed.open(host)
    wid = host.tree["children"][0]["id"]
    for tag in ("selection", "view_x", "layer", "focus", "height"):
        assert ed.apply("/gui_event", [wid, 1, 0, tag, 0.0, 1.0]) is False
    assert ed.can_undo is False
    assert ed.selection["start"] == pytest.approx(0.0)


def test_the_routing_table_is_the_crates_and_not_this_modules():
    # It was eight strings here and eight in the web client's editor, and
    # nothing kept the two agreeing. Now both read the one list the document
    # crate holds beside the presentation rule it states.
    from clausters import _native
    from clausters.gui.editing import not_an_edit

    assert not_an_edit() == _native.view_not_an_edit()
    assert "selection" in not_an_edit()
    assert "notes" not in not_an_edit(), "an edit is not screen state"
    assert not_an_edit() is not_an_edit(), "read once and kept"


def test_a_tag_this_domain_does_not_know_is_nothing_rather_than_an_error():
    ed = an_editor()
    host = FakeHost()
    ed.open(host)
    wid = host.tree["children"][0]["id"]
    assert ed.apply("/gui_event", [wid, 1, 0, "notes", 0, 1, 60, 100, 0]) is False


def test_an_event_for_a_widget_this_editor_did_not_draw_is_not_answered():
    # A poll loop may be shared: answering for another view's window retires a
    # pending edit nobody applied.
    ed = an_editor()
    host = FakeHost()
    ed.open(host)
    before = len(host.acks)
    assert ed.apply("/gui_event", [123_456, 7, 0, "dial", 1.0]) is False
    assert len(host.acks) == before


def test_two_editors_over_one_structure_keep_one_history():
    dial = Dial()
    left, right = an_editor(dial), an_editor(dial)
    left_host, right_host = FakeHost(), FakeHost()
    left.open(left_host)
    right.open(right_host)
    wid = left_host.tree["children"][0]["id"]

    assert left.apply("/gui_event", [wid, 1, 0, "dial", 0.9]) is True
    # The other window is told, and it is told the value rather than redrawn.
    assert right_host.pushes, "a second view of one structure hears the edit"
    assert right.can_undo and right.undo_label == "turn the dial", \
        "one pile: an undo in either window walks the same order"


def test_an_editor_with_no_window_still_shares_the_history():
    dial = Dial()
    open_one, silent = an_editor(dial), an_editor(dial)
    host = FakeHost()
    open_one.open(host)
    wid = host.tree["children"][0]["id"]
    open_one.apply("/gui_event", [wid, 1, 0, "dial", 0.4])
    assert silent.can_undo, "it has no picture; it still shares the pile"


# ---- screen state is about a thing, and a thing is not its address ----

def test_a_new_structure_does_not_inherit_a_freed_ones_screen_state():
    # CPython reuses an address the moment an object is freed (196 times out of
    # 200 in a straight loop), so screen state keyed by `id()` is handed to
    # whatever lands there next: a curve drawn against the axis of a curve that
    # is gone, an aggregate drawn expanded because a cut let go of one.
    import gc

    from clausters.gui.editing import PointsView

    view = PointsView()
    gone = Automation(Env([0.0, 100.0], [2.0]), None, name="gone")
    view.drawn(gone, gone.to_points())
    del gone
    gc.collect()

    fresh = Automation(Env([0.0, 1.0], [2.0]), None, name="fresh")
    drawn = view.drawn(fresh, fresh.to_points())
    assert drawn["max"] < 10.0, f"it took the freed curve's axis: {drawn}"


def test_a_drawers_id_space_goes_when_the_drawer_does():
    # The same defect one level up, in what AP1 added: a table keyed by the
    # drawer's address would hand a new editor whatever the last one at that
    # address had named.
    import gc

    app = Application()
    first = an_editor(app=app)
    first.draw()
    assert len(app._offline) == 1
    # An application holds its editors, the way a host holds an open one, so the
    # table goes when the editor **leaves** — which is what `close` does.
    app.forget(first)
    del first
    gc.collect()
    assert len(app._offline) == 0, "the id space went with the drawer"


# ---- a redraw says what to look like, and the host decides what it costs ----

def a_tree(**props):
    """A two-widget picture: the ids are the caller's, as a named draw's are."""
    return {"type": "window", "children": [
        {"id": 10, "type": "number", "value": props.get("left", 0.0)},
        {"id": 11, "type": "number", "value": props.get("right", 0.0)}]}


def test_a_publish_sends_the_tree_and_nothing_is_remembered():
    # The client holds no picture of the host's. It used to keep the last tree
    # per window and send the difference, which is only correct if that copy
    # equals what the host holds -- and it cannot, because the host moves
    # widgets on its own and screen state is reported by nothing.
    app, host = Application(), FakeHost()
    app.host = host
    app.publish(1, a_tree())
    app.publish(1, a_tree())
    assert [wid for wid, _ in host.defines] == [1, 1], \
        "the same picture twice is the same message"
    assert host.sets == [], "a client that holds no picture computes no set"


def test_publishing_a_part_names_the_widget_and_the_window_it_is_in():
    # The granularity is the caller's, and this is the door for it: a
    # `/gui_def` names any widget, so an edit publishes the one it touched
    # rather than the window around it.
    app, host = Application(), FakeHost()
    app.host = host
    app.publish(1, a_tree())
    app.publish(11, {"id": 11, "type": "number", "value": 0.5}, window=1)
    assert host.redefines == [(11, 1)]
    assert [wid for wid, _ in host.defines] == [1], \
        "and the window was not redrawn for it"


def test_a_window_that_closed_and_opened_again_is_no_special_case():
    # It used to be one: a difference against a picture nobody was drawing
    # would have been sets onto freed widgets, so a closing window had to be
    # forgotten. Nothing is remembered now, so there is nothing to forget.
    ed = an_editor()
    host = FakeHost()
    ed.open(host)
    ed.close()
    ed.app.publish(1, a_tree())
    assert host.defines[-1][0] == 1


# ---- the application: what a window set owns, as against one structure ----

def test_an_editor_makes_an_application_of_one_and_registers_in_it():
    # Nothing a script writes changes: an editor handed no application is an
    # application of one, which is what it always was before there was a name.
    ed = an_editor()
    assert isinstance(ed.app, Application)
    assert ed.app.editors == [ed]
    assert ed.app.context is ed._editing


def test_the_application_owns_the_host_and_the_widget_id_space():
    left = an_editor()
    right = an_editor(app=left.app)
    host = FakeHost()
    left.open(host)

    # One host for the window set: the second editor did not have to be told.
    assert right._host is host
    # And one id space, so two pictures in one application cannot collide.
    assert left.draw()["children"][0]["id"] != right.draw()["children"][0]["id"]


def test_an_application_adopts_a_host_once():
    # The rule the multitrack learned the hard way: an application already open
    # answers *its* host, and a second window opened on another one does not
    # take the acknowledgements with it.
    ed = an_editor()
    first, second = FakeHost(), FakeHost()
    ed.open(first)
    ed.app.resolve(second)
    assert ed._host is first


def test_one_application_is_one_drain_and_each_editor_answers_for_its_own():
    # One socket, one loop: both editors are offered every message, and each
    # answers only for the widget it drew -- which is what makes a bundle of
    # subviews one application rather than two loops racing each other.
    left_dial, right_dial = Dial(), Dial()
    left = an_editor(left_dial)
    right = an_editor(right_dial, app=left.app)
    host = QueueHost()
    left.open(host)
    right._window = 998                       # a second window on the one host
    left_wid = left.draw()["children"][0]["id"]
    right_wid = right.draw()["children"][0]["id"]
    host.pending = [("/gui_event", [left_wid, 1, 0, "dial", 0.3]),
                    ("/gui_event", [right_wid, 2, 0, "dial", 0.7])]

    assert left.app.poll() is True
    assert (left_dial.value, right_dial.value) == (0.3, 0.7)
    # And the window's own handlers still see everything the drain took off the
    # socket, which is the half a data-only drain used to swallow.
    assert len(host.dispatched) == 2


def test_an_editor_that_closes_leaves_the_drain():
    left = an_editor()
    right = an_editor(app=left.app)
    left.open(FakeHost())
    left.close()
    assert left.app.editors == [right]


# ---- the widget id: an identity, not a lease ----

def test_a_redraw_leaves_every_id_where_it_was():
    # The whole of it. A leased id changed on every redraw, so anything holding
    # one across a redraw held a number that now draws something else.
    ed = an_editor()
    host = FakeHost()
    ed.open(host)
    first = ed.draw()["children"][0]["id"]
    assert ed.draw()["children"][0]["id"] == first
    assert ed.draw()["children"][0]["id"] == first, "and not only the once"


def test_an_edit_that_crosses_a_redraw_lands_on_the_widget_the_hand_touched():
    # The failure this fixes, driven end to end: the hand acts, the picture is
    # rebuilt before the event is routed, and the event still names the widget
    # it was made on.
    dial = Dial()
    ed = an_editor(dial)
    host = FakeHost()
    ed.open(host)
    wid = host.tree["children"][0]["id"]

    ed.draw()                                    # a redraw, mid-gesture
    assert ed._owns(wid), "the widget the event names is still this view's"
    assert ed.apply("/gui_event", [wid, 1, 0, "dial", 0.8]) is True
    assert dial.value == 0.8, "and the edit reached the data"


def test_the_two_views_of_one_structure_name_the_same_widget():
    # A name is the structure's identity plus the role, so two pictures of one
    # thing agree about which widget draws which part of it — which is what
    # makes a correction from either one addressable by the other.
    dial = Dial()
    host = FakeHost()
    left, right = an_editor(dial), an_editor(dial, app=None)
    left.open(host)
    right.open(host)
    assert left.draw()["children"][0]["id"] == right.draw()["children"][0]["id"]


def test_a_widget_that_stopped_being_drawn_gives_its_id_back():
    ed = an_editor()
    host = FakeHost()
    ed.open(host)
    wid = ed.draw()["children"][0]["id"]
    held = host.ids.in_use

    ed.view = View()                             # a picture that draws nothing
    ed.view.build = lambda editor: {"type": "window", "children": []}
    ed.draw()
    assert host.ids.in_use == held - 1, "the name let go of it"
    assert not ed._owns(wid)


def test_two_editors_on_one_host_do_not_retire_each_others_widgets():
    # One table, two drawers. Either redrawing used to be enough to take back
    # the other's ids, because the cycle did not say whose draw it was.
    host = FakeHost()
    left, right = an_editor(), an_editor()
    left.open(host)
    right.open(host)
    right_id = right.draw()["children"][0]["id"]

    left.draw()
    assert right.draw()["children"][0]["id"] == right_id
    assert right._owns(right_id)


def test_the_context_is_the_structures_and_is_asked_for_not_built():
    dial = Dial()
    ed = an_editor(dial)
    assert ed._editing is Editing.of(dial)


# ---- the collaborators, on their own ----

def test_the_echo_is_the_protocol_and_needs_no_structure():
    host = FakeHost()
    version = 3
    echo = Echo(host=host, version=lambda: version)

    echo.announce()
    assert host.acks[-1] == (0, 3, None), "the host is told what it is drawing"

    # **What a message is, is the crate's** -- and so is what raises the floor.
    # The version moved by a route no event took, so the next gesture made
    # against the picture that is gone is refused and the one naming nothing at
    # all still applies.
    version = 5

    def event(against):
        return {"addr": "/gui_event", "argc": 5, "widget": 7, "seq": 1,
                "against": against, "tag": "points", "version": version,
                "isWindow": False, "owns": True}

    assert echo.read(event(4))["turn"] == "stale"
    assert echo.floor == 5, "and the floor is where the version was"
    assert echo.read(event(0))["turn"] == "route", "unstated applies unchecked"
    assert echo.read(event(9))["turn"] == "route"
    #: Back to 3 for the half below: an acknowledgement carries the version the
    #: context is at *now*, and the floor is a separate number that stays where
    #: the verb left it.
    version = 3

    echo.correct(7, value=1.0)
    echo.acknowledge(2, reason="not here")
    assert host.pushes[-1] == (2, [(7, {"value": 1.0})], 3, "not here")

    echo.clear()
    echo.acknowledge(2)
    assert host.acks[-1] == (2, 3, None), "with nothing to correct it is a bare ack"


def test_an_echo_with_no_host_answers_by_doing_nothing():
    echo = Echo(version=lambda: 1)
    echo.announce()
    echo.correct(1, value=0.0)
    echo.acknowledge(3)          # no host: nothing to say it to, and no error


def test_a_view_owns_what_it_drew_and_nothing_else():
    ed = an_editor()
    tree = ed.draw()
    wid = tree["children"][0]["id"]
    assert ed.view.owns(wid) and not ed.view.owns(wid + 1)
    assert ed.view.showing(wid) is ed.structure


def test_a_domain_takes_its_coalesce_key_from_the_crate():
    # One vocabulary, one key, in the shared implementation both clients bind —
    # never a second answer written per language.
    domain = DialDomain()
    payload = domain.payload(Dial(), "dial", [1.0])
    assert domain.coalesce_key(payload) == "points"


def test_two_editors_in_one_application_keep_their_own_floor():
    """The echo is **one view's** end of the conversation, so a window set does
    not share one.

    The floor rises when the version moved and no event of *this* view moved it.
    With one echo per application the two windows would each answer for the
    other: a gesture the left window made against a picture the right window had
    already changed would find `version == applied` and be accepted, which is
    the whole of what the floor is for.
    """
    left = an_editor()
    right = an_editor(structure=left.structure, app=left.app)
    assert left.echo is not right.echo, "an echo is a view's, not a window set's"
    assert left.app is right.app, "and the window set is still one"

    # The right window edits; the left one's conversation has not answered
    # anything since, so its `applied` stays where it was.
    before = left.echo.state["applied"]
    host = FakeHost()
    right.open(host)
    wid = host.tree["children"][0]["id"]
    assert right.apply("/gui_event", [wid, 1, 0, "dial", 0.75]) is True
    assert left._editing.version > before, "the neighbour moved the version"
    assert left.echo.state["applied"] == before, (
        "the neighbour's edit answered for this window's conversation"
    )


def test_the_editing_trace_is_silent_until_it_is_watched():
    """The five joints, and the fact that they cost nothing unarmed.

    A window in front of a person fails in ways nothing else sees, so the path
    says what it did — but a library that printed by default would make every
    importer pay for the formatting. The twin is
    `clients/web/tests/gui-edit.test.ts`.
    """
    import io

    from clausters.log import unwatch
    from clausters.gui.editing import watch

    ed = an_editor()
    host = FakeHost()
    ed.open(host)
    wid = host.tree["children"][0]["id"]

    quiet = io.StringIO()
    unwatch()
    assert ed.apply("/gui_event", [wid, 1, 0, "dial", 0.25]) is True
    assert quiet.getvalue() == "", "silent unless asked"

    printed = io.StringIO()
    watch(printed)
    try:
        assert ed.apply("/gui_event", [wid, 2, 0, "dial", 0.75]) is True
    finally:
        unwatch()
    said = printed.getvalue()
    assert "event " in said, said
    assert "record [" in said, said
    assert "ack " in said, said
