"""`AudioEditor`: a take edited as a list of parts over takes it never writes.

What is checked is this client's half -- the steps the crate answers are walked
against the take's server, the buffers a turn needs are handed over before it,
and the takes the history lets go of are freed. What each gesture does to the
list is the crate's and is tested there.
"""

import pytest

from clausters.gui import edit
from clausters.gui.editing import AudioEditor, Editing, measures

SR = 48_000.0


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.trees: list = []
        self.next = 20_000

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
        pass

    def _set_closed_handler(self, id, func):
        pass

    def subscribe(self, func):
        return func

    def unsubscribe(self, func):
        pass

    looping = False
    loop = None


class FakeAllocator:
    """Buffer numbers, handed out from 100 and taken back."""

    def __init__(self):
        self.next = 100
        self.freed: list = []

    def alloc(self) -> int:
        self.next += 1
        return self.next

    def free(self, bufnum: int) -> None:
        self.freed.append(bufnum)


class FakeServer:
    """Every message a take's steps send, in order, each answered the way the
    server answers it."""

    def __init__(self):
        self.sent: list = []
        self.buffers = FakeAllocator()

    def _bulk_chunk(self, timeout=None) -> int:
        return 8192

    def send_msg(self, addr, *args):
        self.sent.append((addr, args))

    def request(self, addr, *args, expect=None, timeout=None):
        self.send_msg(addr, *args)
        if addr == "/server_sync":
            return "/server_sync.reply", [int(args[0])]
        return "/done", [addr, int(args[0])]

    def addrs(self) -> list:
        return [addr for addr, _ in self.sent]


class FakeBuffer:
    """The take: a number, a shape, and the server it is on."""

    def __init__(self, frames=100, channels=1):
        self.bufnum = 7
        self.frames = frames
        self.channels = channels
        self.sample_rate = SR
        self.path = None
        self.server = FakeServer()

    def set_samples(self, samples, start=0, **kwargs):
        raise AssertionError("an audio editor never writes the take")


def opened(take, **options):
    context = Editing()
    editor = AudioEditor(take, context=context, **options)
    host = FakeHost()
    editor.open(host)
    return editor, host, host.trees[0]["children"][0]["id"], context


def test_the_window_draws_a_join_over_a_private_copy_of_the_take():
    take = FakeBuffer()
    editor, _host, _wid, _context = opened(take)
    display = editor.buffer.bufnum
    copy = [args for addr, args in take.server.sent if addr == "/buffer_gen"][0]
    assert (copy[1], copy[3]) == ("copy", take.bufnum), "copied out of the take"
    stitched = [args for addr, args in take.server.sent if addr == "/buffer_stitch"]
    assert stitched and stitched[0][0] == display
    assert stitched[0][3] == copy[0], "it reads the copy"
    assert editor.buffer.frames == 100
    assert "/buffer_setRange" not in take.server.addrs(), "the take is never written"


def test_a_stroke_writes_a_new_take_and_the_join_reads_it():
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    take.server.sent.clear()
    assert editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 40,
                                       [0.5, -0.5], [0.0, 0.0]]) is True
    assert take.server.addrs() == ["/buffer_alloc", "/server_sync",
                                   "/buffer_setRange", "/buffer_stitch"]
    new = take.server.sent[0][1][0]
    copy = editor.parts[0]["source"]["source"]
    assert [(p["source"]["source"], p["source"]["range"]["start"])
            for p in editor.parts] == [(copy, 0), (new, 0), (copy, 42)]


def test_a_take_the_history_cannot_reach_is_freed():
    take = FakeBuffer()
    editor, _host, wid, context = opened(take)
    editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]])
    first = editor.parts[1]["source"]["source"]
    assert editor.undo() is True
    assert first not in take.server.buffers.freed, "a redo can still find it"
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 30, [0.5], [0.0]])
    assert first in take.server.buffers.freed
    assert ("/buffer_free", (first,)) in take.server.sent


def test_the_history_is_held_to_its_byte_limit():
    take = FakeBuffer()
    editor, _host, wid, context = opened(take, history_bytes=4)
    for seq in (1, 2, 3):
        editor.apply("/gui_event", [wid, seq, context.version, "draw", 0, 10,
                                    [0.5], [0.0]])
    assert len(take.server.buffers.freed) == 1, "the oldest take only the history held"
    assert editor.can_undo


def test_a_cut_moves_no_samples():
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    take.server.sent.clear()
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0])
    assert take.server.addrs() == ["/buffer_stitch"]
    assert editor.buffer.frames == 80
    assert editor.undo() is True
    assert editor.buffer.frames == 100


def test_a_take_past_the_resident_budget_goes_to_disk_and_comes_back():
    take = FakeBuffer()
    editor, _host, wid, context = opened(take, resident_bytes=0, scratch="/scratch")
    take.server.sent.clear()
    for seq in (1, 2):
        editor.apply("/gui_event", [wid, seq, context.version, "draw", 0, 10,
                                    [0.5], [0.0]])
    first = take.server.sent[take.server.addrs().index("/buffer_alloc")][1][0]
    assert ("/buffer_write", (first, f"/scratch/take-{first}.wav", "wav", "float")) \
        in take.server.sent
    take.server.sent.clear()
    assert editor.undo() is True
    assert take.server.addrs()[:2] == ["/buffer_allocRead", "/buffer_stitch"]


def test_a_save_writes_the_edited_take_over_its_file_or_as_another():
    take = FakeBuffer()
    take.path = "/takes/a.wav"
    editor, _host, _wid, _context = opened(take)
    take.server.sent.clear()
    assert editor.save() == "/takes/a.wav"
    assert take.server.sent == [("/buffer_write", (editor.buffer.bufnum,
                                                   "/takes/a.wav", "wav", "float"))]
    assert editor.save("/takes/b.wav", sample_format="int24") == "/takes/b.wav"
    assert editor.save() == "/takes/b.wav"


def test_a_save_over_a_buffer_rewrites_it_or_writes_a_new_one():
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0])
    take.server.sent.clear()
    saved = editor.save()
    assert saved.bufnum == take.bufnum and saved.frames == 80
    assert take.server.addrs() == ["/buffer_alloc", "/server_sync", "/buffer_gen"]
    fresh = editor.save(buffer=True)
    assert fresh.bufnum not in (take.bufnum, editor.buffer.bufnum)
    assert editor.save().bufnum == fresh.bufnum, "a later save writes there"


def test_edit_opens_a_buffer_in_the_audio_editor():
    assert isinstance(edit(FakeBuffer(), open=False, context=Editing()), AudioEditor)


def test_a_takes_window_is_composed_by_the_crate():
    take = FakeBuffer(frames=8, channels=2)
    editor, host, wid, _context = opened(take, title="take")
    tree = host.trees[0]
    assert (tree["type"], tree["title"], tree["flow"]) == ("window", "take", "col")
    picture = tree["children"][0]
    assert picture["type"] == "signal" and picture["id"] == wid
    assert (picture["buffer"], picture["channels"]) == (editor.buffer.bufnum, 2)
    assert picture["measure"] == "peak rms"
    assert picture["gestures"] == {"drag": "select", "alt": "draw", "ctrl": "sample"}
    assert editor.view.props(editor, wid) == {"reload": 1}


def test_a_refused_measure_stack_keeps_the_one_the_picture_had():
    editor = AudioEditor(FakeBuffer(), layers=("peak",), context=Editing())
    editor.layers = ("rms", "peak")
    assert editor.layers == ("rms", "peak")
    with pytest.raises(ValueError, match="'loud'"):
        editor.layers = ("loud",)
    assert editor.layers == ("rms", "peak")
    with pytest.raises(ValueError, match="measures something"):
        measures(())


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__]))
