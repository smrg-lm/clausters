"""A node's generic editor: the core's window from a def's controls, and the
text verbs it reads a value with."""

from dataclasses import asdict

from clausters import _native
from clausters.defs import Node, SynthDef, control, out, sine
from clausters.defs.info import ControlInfo
from clausters.gui import edit
from clausters.gui.editing import NodeEditor


def _rows(answer: dict) -> list:
    return [row for row in answer["contents"]["children"] if row["type"] == "layout"]


def test_a_def_s_controls_become_a_window_bound_to_the_node():
    """What a SynthDef declares is what the window draws: a slider over the
    declared range, a toggle for a switch, the conventional range for a name
    that declares none -- each bound straight to the node."""
    freq = control("freq", 660.0, min=220.0, max=1760.0, warp="exp", unit="Hz")
    gate = control("vibrato", 1.0, min=0.0, max=1.0, step=1.0)
    amp = control("amp", 0.1)
    sd = SynthDef("tone", out(0, sine(freq * gate) * amp))
    answer = _native.node_editor(7, "tone", [asdict(c) for c in sd.controls])
    assert answer["window"]["title"] == "tone (7)"
    widgets = {row["children"][1]["name"]: row["children"][1] for row in _rows(answer)}
    slider = widgets["control:freq"]
    assert (slider["type"], slider["min"], slider["max"], slider["warp"]) == (
        "slider", 220.0, 1760.0, "exp")
    assert slider["bind"] == ["/node_set", 7, "freq"]
    assert widgets["control:vibrato"]["type"] == "toggle"
    assert (widgets["control:amp"]["min"], widgets["control:amp"]["max"]) == (0.0, 1.0)
    texts = [row["children"][2] for row in _rows(answer)]
    assert texts[0]["text"] == "660 Hz"
    assert texts[0]["bind"] == ["/node_setText", 7, "freq"]


def test_a_search_and_a_page_are_the_core_s():
    controls = [asdict(ControlInfo(f"p{i}", 0.5, min=0.0, max=1.0)) for i in range(40)]
    first = _native.node_editor(7, "big", controls)
    assert first["pages"] == ["1-16 of 40", "17-32 of 40", "33-40 of 40"]
    last = _native.node_editor(7, "big", controls, page=99)
    assert last["page"] == 2 and len(_rows(last)) == 8
    found = _native.node_editor(7, "big", controls, query="P39")
    assert [row["children"][1]["name"] for row in _rows(found)] == ["control:p39"]


def test_edit_of_a_node_is_its_generic_editor():
    editor = edit(Node(42, server=object()), open=False, title="mine")
    assert isinstance(editor, NodeEditor)
    assert editor.closed and editor.window is None


class _Server:
    """Records what is sent and answers a request with one reply."""

    def __init__(self, reply=None):
        self.sent, self.reply = [], reply

    def send_msg(self, addr, *args):
        self.sent.append((addr, args))

    def request(self, addr, *args, **options):
        self.sent.append((addr, args))
        assert options["match"](self.reply[1])
        return self.reply


def test_the_text_verbs_are_the_wire_s():
    server = _Server(("/node_setText", [42, "freq", "880 Hz", "amp", "0.1"]))
    node = Node(42, server=server)
    node.set_text({"freq": "880 Hz"})
    assert node.get_text("freq", "amp") == {"freq": "880 Hz", "amp": "0.1"}
    assert server.sent == [("/node_setText", (42, "freq", "880 Hz")),
                           ("/synth_getText", (42, "freq", "amp"))]
