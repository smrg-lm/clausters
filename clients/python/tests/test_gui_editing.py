"""The generic editor and its collaborators (`clausters.gui.editing`).

`Editor` edits **one structure** and imports nothing from the arrangement, so
this drives one with no composition anywhere: a plain object, a domain that
says what a gesture means to it, and a view that draws it. What is checked is
the orchestration — the gesture becomes a payload, the payload becomes an
entry, the entry inverts — and the two collaborators that are testable with no
data at all.
"""

import pytest

from clausters.gui.editing import Domain, Echo, Editing, Editor, View

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
        wid = editor._new_id()
        self.register(wid, editor.structure)
        return {"type": "window", "children": [
            {"id": wid, "type": "number", "value": editor.structure.value}]}

    def props(self, editor, widget_id: int) -> dict:
        return {"value": editor.structure.value}


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.pushes: list = []
        self.next = 20_000

    def alloc_id(self) -> int:
        self.next += 1
        return self.next

    def open(self, tree, id=None):
        self.tree = tree
        return 999

    def ack(self, seq, doc_version=0, reason=None):
        self.acks.append((seq, doc_version, reason))

    def push(self, seq, *corrections, doc_version=0, reason=None):
        self.pushes.append((seq, list(corrections), doc_version, reason))

    def define(self, wid, tree):
        return wid

    def poll(self, timeout=0.0):
        return None

    def dispatch(self, *msg):
        pass


def an_editor(structure=None):
    return Editor(structure or Dial(), sample_rate=SR, tempo=2.0,
                  domain=DialDomain(), view=DialView())


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
