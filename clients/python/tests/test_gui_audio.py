"""`AudioEditor`: a take edited as a list of parts over takes it never writes.

What is checked is this client's half -- the steps the crate answers are walked
against the take's server, the buffers a turn needs are handed over before it,
and the takes the history lets go of are freed. What each gesture does to the
list is the crate's and is tested there.
"""

import types

import pytest

from clausters import _native
from clausters.gui import edit
from clausters.gui.editing import AudioEditor, Editing, measures

SR = 48_000.0


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.trees: list = []
        self.next = 20_000
        self.clock = "device"

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

    def head_clock(self, id, name, transport=0):
        self.clock = name
        self.clock_of = int(getattr(id, "id", id))

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
        self.ids = _native.IdSpaces(max_nodes=8192, audio_buses=1024, outputs=2,
                                    control_buses=16384, buffers=4096)
        #: Whether the transport rolls, as a `/transport_query` answers.
        self.playing = False

    def _bulk_chunk(self, timeout=None) -> int:
        return 8192

    def _ensure_recycler(self) -> None:
        pass

    def query_info(self, timeout=None):
        return types.SimpleNamespace(nominal_sample_rate=SR)

    def transport_at(self, transport):
        return types.SimpleNamespace(state=lambda: {"playing": self.playing})

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


def opened(take, **options):
    context = Editing()
    editor = AudioEditor(take, context=context, **options)
    host = FakeHost()
    editor.open(host)
    return editor, host, picture(host.trees[0])["id"], context


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


def test_the_window_opens_with_both_cursors_on_the_transport_clock():
    editor, host, _wid, _context = opened(FakeBuffer())
    take = picture(host.trees[0])
    assert take["axes"]["x"]["cursor"] == 0.0, "the position cursor, placed"
    assert take["axes"]["x"]["playhead_at"] == 0.0, "the play cursor, anchored"
    assert host.clock == "transport", "drawn from the transport's position"
    assert host.clock_of == 901, "the editor's own window"


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
    assert take.server.addrs() == ["/buffer_stitch", "/node_set"], \
        "the join stitched, and the readers' window set to its new length"
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
    assert (tree["type"], tree["title"]) == ("window", "take")
    assert tree["children"][1]["flow"] == "row", "the take, and its level beside it"
    taken = picture(tree)
    assert taken["type"] == "signal" and taken["id"] == wid
    assert (taken["buffer"], taken["channels"]) == (editor.buffer.bufnum, 2)
    assert taken["measure"] == "peak rms"
    assert taken["gestures"] == {"drag": "select", "alt": "draw", "ctrl": "sample"}
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


def test_the_take_sounds_through_the_editors_own_nodes():
    """**The editor's nodes are made with the window**: the structure on the
    editor's transport, one play graph with a reader per channel of the join,
    and the level meter beside the take once the playback says where it
    writes."""
    take = FakeBuffer(channels=2)
    editor, host, _wid, _context = opened(take)
    sent = take.server.addrs()
    for addr in ("/transport_follow", "/transport_group", "/transport_fade"):
        assert addr in sent, addr
    readers = [args for addr, args in take.server.sent if addr == "/graph_addSlot"]
    assert len(readers) == 2, "a reader per channel"
    assert host.clock == "transport"
    window = host.trees[0]
    assert window["plays"] is True, "the space bar is the editor's"
    meter = next(c for c in window["children"][1]["children"] if c["type"] == "meter")
    assert meter["type"] == "meter" and meter["rate"] == "control"
    assert meter["channels"] == 2


def test_the_space_bar_plays_and_a_second_press_stops_back_at_the_cursor():
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    window = editor._window
    take.server.sent.clear()
    editor.apply("/gui_event", [wid, 1, 0, "locate", 40])
    editor.apply("/gui_event", [int(window), 2, 0, "play", 0])
    end = [args for addr, args in take.server.sent if addr == "/transport_end"]
    assert [int(a.value) if hasattr(a, "value") else a for a in end[-1]] == [1, 100, 40]
    assert take.server.addrs()[-1] == "/transport_play"
    take.server.sent.clear()
    take.server.playing = True
    editor.apply("/gui_event", [int(window), 3, 0, "play", 0])
    assert take.server.addrs()[:2] == ["/transport_stop", "/transport_locateSample"]


def test_closing_the_window_frees_the_editors_nodes():
    take = FakeBuffer()
    editor, _host, _wid, _context = opened(take)
    take.server.sent.clear()
    editor.close()
    assert "/node_free" in take.server.addrs()


def test_placing_the_cursor_cues_a_stopped_transport_there():
    """**The play cursor goes with the position cursor** while nothing plays;
    rolling, the mark moves and the music does not."""
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    take.server.sent.clear()
    editor.apply("/gui_event", [wid, 1, 0, "locate", 40])
    located = [args for addr, args in take.server.sent if addr == "/transport_locateSample"]
    assert [a.value if hasattr(a, "value") else a for a in located[-1]] == [1, 40]
    take.server.sent.clear()
    take.server.playing = True
    editor.apply("/gui_event", [wid, 2, 0, "locate", 60])
    assert "/transport_locateSample" not in take.server.addrs()


def test_a_loop_follows_the_selection_redrawn_while_it_plays():
    """**A selection redrawn while its loop plays moves the loop**: every move
    of the sweep is the span, and the locate it is let go with puts the head
    on the span's start. A pass that runs to its end is left as it was
    played."""
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    window = int(editor._window)
    server = take.server

    def sent(addr):
        return [[int(a.value) if hasattr(a, "value") else a for a in args]
                for at, args in server.sent if at == addr]

    editor.apply("/gui_event", [wid, 1, 0, "selection", 10, 20])
    editor.apply("/gui_event", [window, 2, 0, "play", 1])
    server.playing = True
    server.sent.clear()
    editor.apply("/gui_event", [wid, 3, 0, "selection", 40, 0])
    editor.apply("/gui_event", [wid, 4, 0, "selection", 40, 30])
    assert sent("/transport_loop")[-1] == [1, 40, 70], "the span follows the hand"
    assert not sent("/transport_locateSample"), "and the head does not"
    editor.apply("/gui_event", [wid, 5, 0, "locate", 40])
    assert sent("/transport_locateSample")[-1] == [1, 40], "let go: into the span"

    # Not looping, the pass keeps the end it was played with.
    editor.apply("/gui_event", [window, 6, 0, "loop", 0])
    server.sent.clear()
    editor.apply("/gui_event", [wid, 7, 0, "selection", 5, 10])
    editor.apply("/gui_event", [wid, 8, 0, "locate", 5])
    assert not [addr for addr in server.addrs() if addr.startswith("/transport_")]


def _transported(take):
    """The take's server answering a real `Transport`, rolling as it says."""
    from clausters.defs import Transport

    server = take.server
    held = {}

    class Rolling(Transport):
        def state(self):
            return {"playing": server.playing}

    server.transport_at = lambda n: held.setdefault(n, Rolling(server, n))
    return server


def test_the_take_s_transport_plays_its_span_and_keeps_the_switch():
    take = FakeBuffer()
    server = _transported(take)
    editor, host, wid, _context = opened(take)
    host.set = lambda w, **props: host.__dict__.setdefault("props", {}).setdefault(w, {}).update(props)
    transport = editor.transport
    assert transport is editor.transport, "one transport, one object"
    transport.loop(10 / SR, 30 / SR)
    assert transport.span == (10 / SR, 30 / SR) and transport.looping
    assert host.props[wid]["sel_start"] == 10 and host.props[wid]["sel_len"] == 20
    assert host.props[wid]["looping"] == 1, "the window's L switch"
    server.sent.clear()
    transport.play()
    loops = [args for addr, args in server.sent if addr == "/transport_loop"]
    assert loops and [a.value for a in loops[-1][-2:]] == [10, 30], "the span, looped"
    assert editor.selected.start == 10, "what is marked is the range"
    editor.unselect()
    assert transport.span is None and editor.selected is None
    editor.apply("/gui_event", [wid, 9, 0, "selection", 20, 40])
    assert transport.span == (20 / SR, 60 / SR), "a sweep is the transport's span"
