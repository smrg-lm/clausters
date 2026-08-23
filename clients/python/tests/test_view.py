"""The GUI node as an object: `clausters.gui.view.View`.

A builder used to return a bare ``dict``; it now returns a `View`, which is a
``dict`` — so the document is unchanged — that also carries the client-side name
index and knows how to open itself.
"""

import json

import pytest

from clausters.gui import View, knob, label, layout, slider, view
from clausters.gui.guidef import to_json


class FakeHost:
    """Records what a `View.open` would send."""

    def __init__(self):
        self.opened = []

    def open(self, tree, *blobs, id=None):
        self.opened.append((tree, blobs, id))
        return "handle"


def test_a_builder_returns_a_view_that_is_still_a_dict():
    v = knob(name="freq", min=110.0, max=880.0)
    assert isinstance(v, View)
    assert isinstance(v, dict)
    assert v == {"type": "knob", "name": "freq", "min": 110.0, "max": 880.0}
    assert v.type == "knob"
    assert v.name == "freq"


def test_the_document_is_byte_identical_to_the_plain_dict_form():
    v = view(layout(knob(name="freq"), slider(name="amp"), flow="col"), title="t")
    plain = {
        "type": "window",
        "title": "t",
        "children": [
            {"type": "layout", "flow": "col",
             "children": [{"type": "knob"}, {"type": "slider"}]},
        ],
    }
    assert v.to_json() == json.dumps(plain)
    assert to_json(v) == v.to_json()


def test_a_name_is_found_anywhere_in_the_view():
    v = view(layout(layout(knob(name="freq")), slider(name="amp")))
    assert v.names() == ["freq", "amp"]
    assert v.find("freq").type == "knob"
    assert v.find("amp").type == "slider"


def test_an_unknown_name_names_what_is_there():
    v = view(knob(name="freq"))
    with pytest.raises(KeyError, match="freq"):
        v.find("cutoff")


def test_a_duplicate_name_is_refused_where_the_tree_is_built():
    with pytest.raises(ValueError, match="duplicate widget name 'freq'"):
        layout(knob(name="freq"), slider(name="freq"))


def test_a_duplicate_name_is_refused_across_subtrees_too():
    left = layout(knob(name="freq"))
    right = layout(slider(name="freq"))
    with pytest.raises(ValueError, match="duplicate widget name 'freq'"):
        view(left, right)


def test_a_nested_view_keeps_its_names_to_itself():
    v = view(layout(view(knob(name="freq"), name="osc1"),
                    view(knob(name="freq"), name="osc2")))
    assert v.names() == ["osc1", "osc2"]          # not "freq", twice
    assert v.find("osc1").find("freq").type == "knob"
    with pytest.raises(KeyError):
        v.find("freq")


def test_the_bracket_is_the_dict_key_not_the_name():
    v = knob(name="freq", min=1.0)
    assert v["type"] == "knob"
    assert v["min"] == 1.0
    assert "min" in v and "freq" not in v         # `in` stays a props check


def test_a_root_that_is_not_a_window_is_framed_in_one():
    """A view with no parent is a window. Only a `window`-rooted def becomes an
    OS window on the wire, so a bare root is framed here -- and the frame is
    invisible: the handle still resolves the tree's names."""
    from clausters.gui import GuiHost

    host = GuiHost("127.0.0.1", 57993)
    host._osc = _Recorder()
    tree = layout(knob(name="freq"), flow="col")
    win = tree.open(host=host)

    sent = json.loads(host._osc.sent[0][2])
    assert sent["type"] == "window" and sent["hug"] == 1
    assert [c["type"] for c in sent["children"]] == ["layout"]
    assert win["freq"].id >= 1000
    assert tree.type == "layout", "the caller's tree is not the frame"


def test_a_lone_control_opens_as_a_window_that_is_that_control():
    from clausters.gui import GuiHost

    host = GuiHost("127.0.0.1", 57992)
    host._osc = _Recorder()
    knob(name="freq", min=110.0, max=880.0).open(host=host)

    sent = json.loads(host._osc.sent[0][2])
    assert sent["type"] == "window"
    assert [c["type"] for c in sent["children"]] == ["knob"]


def test_window_is_the_older_spelling_of_view():
    from clausters.gui import window

    assert window is view


class _Recorder:
    def __init__(self):
        self.sent = []

    def start(self):
        return self

    def send_msg(self, target, *args):
        self.sent.append(args)


def test_a_view_opens_itself_on_the_host_it_is_given():
    host = FakeHost()
    v = view(label("hi"))
    assert v.open(b"blob", id=7, host=host) == "handle"
    (tree, blobs, id), = host.opened
    assert tree is v and blobs == (b"blob",) and id == 7


def test_a_view_with_no_host_opens_on_the_ambient_one():
    """The ambient rule the other visual verbs already follow: a registered
    host wins outright, so `view.open()` needs no argument."""
    from clausters.gui import set_ambient_host

    host = FakeHost()
    previous = set_ambient_host(host)
    try:
        view(label("hi")).open()
    finally:
        set_ambient_host(previous)
    assert len(host.opened) == 1
