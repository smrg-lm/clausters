"""The generic editor and its collaborators (`clausters.gui.editing`).

`Editor` edits **one structure** and imports nothing from the arrangement, so
this drives one with no composition anywhere: a plain object, a domain that
says what a gesture means to it, and a view that draws it. What is checked is
the orchestration — the gesture becomes a payload, the payload becomes an
entry, the entry inverts — and the two collaborators that are testable with no
data at all.
"""

import pytest

from clausters.gui.editing import (Application, Domain, Echo, Editing,
                                   Editor, View)
from clausters.gui.ids import GuiIdAllocator

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

    def define(self, wid, tree):
        return wid

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
    return Editor(structure or Dial(), sample_rate=SR, tempo=2.0,
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

    # Unstated applies unchecked; anything under the floor is overtaken.
    echo.floor = 5
    assert echo.stale(0) is False
    assert echo.stale(4) is True
    assert echo.stale(9) is False

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
