"""`AudioEditor`: a take edited as a list of parts over takes it never writes.

What is checked is this client's half -- the steps the crate answers are walked
against the take's server, the buffers a turn needs are handed over before it,
and the takes the history lets go of are freed. What each gesture does to the
list is the crate's and is tested there.
"""

import types
import json

import pytest

from clausters import _native
from clausters.gui.ids import GuiIdAllocator
from clausters.gui import edit
from clausters.gui.editing import AudioEditor, Editing, measures
from clausters.errors import CommandError

SR = 48_000.0


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.trees: list = []
        self.ids = GuiIdAllocator(base=20_000)
        self.clock = "device"

    def alloc_id(self) -> int:
        return self.ids.alloc()

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

    def set(self, id, **props):
        self.__dict__.setdefault("props", {}).setdefault(id, {}).update(props)

    def clipboard(self, document):
        self.clip = document

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
    take = host.trees[0]["children"][1]["children"][1]["children"][0]
    assert take["type"] == "pane"
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


def test_a_cut_copies_before_removing_samples():
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    take.server.sent.clear()
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0])
    assert take.server.addrs()[:4] == ["/buffer_alloc", "/server_sync", "/buffer_gen", "/buffer_stitch"], \
        "the join stitched, the readers' window set to its new length, and the " \
        "transport cued to where the span was"
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
    writes = [args for addr, args in take.server.sent if addr == "/buffer_write"]
    assert any(args[0] == first and args[1].startswith("/scratch/take-")
               and args[1].endswith(f"-{first}.wav") and args[2:] == ("wav", "float")
               for args in writes)
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
    meter = next(c for c in window["children"][1]["children"][1]["children"] if c["type"] == "meter")
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


def test_visual_layers_use_shared_outcomes_for_calls_and_menu_events():
    editor = AudioEditor(FakeBuffer(), context=Editing())
    host = FakeHost()
    editor.open(host)
    editor.hide_layer("peak")
    assert editor.layers[0]["visible"] is False
    editor.show_layer("peak")
    editor.layer_alpha("peak", 0.3)
    editor.solo_layer("peak")
    assert editor.layers[0]["alpha"] == 0.3
    assert editor.layers[0]["solo"] is True
    editor.solo_layer("peak")
    assert host.acks[-1][1][0][1].get("layers")
    editor.view_spectrogram()
    assert editor.layers == ("spectrogram",)
    before = editor.layers
    with pytest.raises(ValueError, match="different domains"):
        editor.show_layer("peak")
    assert editor.layers == before
    editor.view_wave()
    assert editor.layers == ("peak", "rms")
    assert len(host.trees) == 1, "an alternative is a prop, not a new window"


def test_both_views_stack_the_spectrogram_in_a_lane_under_the_take():
    editor = AudioEditor(FakeBuffer(), context=Editing())
    host = FakeHost()
    editor.open(host)
    wid = next(iter(editor.view.widgets))
    editor.apply("/gui_event", [wid, 1, 0, "view_both"])
    assert editor.layers == ("peak", "rms")
    assert len(host.trees) == 2, "the window is drawn again with a second lane"
    lanes = host.trees[-1]["children"][1]["children"][1]["children"][0]
    assert lanes["type"] == "layout"
    pane, lane = lanes["children"]
    assert pane["type"] == "pane"
    assert lane["measure"] == "spectrogram"
    assert lane["link"] == pane["id"]
    editor.view_wave()
    assert len(host.trees) == 3
    assert host.trees[-1]["children"][1]["children"][1]["children"][0]["type"] == "pane"


def test_edit_layer_is_the_panes_body_and_not_the_visual_stack():
    editor, host, wid, _context = opened(FakeBuffer())
    pane = host.trees[0]["children"][1]["children"][1]["children"][0]
    assert pane["type"] == "pane"
    assert pane["children"][0]["id"] == wid
    assert pane["axes"]["x"]["unit"] == "time"
    assert editor.layer == "take"
    editor.layer = "placement"
    assert editor.layer == "placement"
    assert host.acks[-1][1] == [(pane["id"], {"layer": "placement"})]
    editor.layer = "take"
    assert editor.layers == ("peak", "rms")
    with pytest.raises(ValueError, match="no 'points:1' body"):
        editor.layer = "points:1"
    assert editor.layer == "take"


def test_gain_curve_edits_and_apply_share_the_take_history():
    take = FakeBuffer()
    editor, host, _wid, context = opened(take)
    original = editor.parts
    duration = 100 / SR
    points = [0, 0.25, 5, -3, duration, 0.75, 1, 0]
    editor.show_envelope()
    assert editor.layer == "points"
    take.server.sent.clear()
    assert editor.set_envelope(points)
    assert editor.envelope == points
    assert "/buffer_gain" not in take.server.addrs()
    assert "/buffer_setRange" not in take.server.addrs()
    assert editor.apply_envelope()
    gains = [args for addr, args in take.server.sent if addr == "/buffer_gain"]
    assert gains and gains[0][0] not in (take.bufnum, editor.buffer.bufnum)
    assert gains[0][1:7] == (0, 100, 0.25, 0.75, 5, -3.0)
    assert editor.envelope[1] == 1
    assert editor.undo()
    assert editor.parts == original
    assert editor.envelope == points
    assert editor.undo()
    assert editor.envelope[1] == 1
    assert editor.redo()
    assert editor.redo()
    assert editor.envelope[1] == 1
    assert context.version > 1
    assert host.acks


def test_invalid_gain_quads_leave_history_and_samples_unchanged():
    editor, _host, _wid, context = opened(FakeBuffer())
    before, version = editor.envelope, context.version
    with pytest.raises(ValueError, match="complete"):
        editor.set_envelope([0, 1, 1])
    assert editor.envelope == before
    assert context.version == version
    assert not editor.apply_envelope(), "unity does not make another take"


def test_preview_switches_keep_samples_and_history_and_release_the_insert():
    take = FakeBuffer()
    editor, host, wid, context = opened(take)
    editor.set_envelope([0, 0.25, 1, 0, 100 / SR, 0.75, 1, 0])
    before, version = editor.parts, context.version
    take.server.sent.clear()
    editor.preview_on()
    assert editor.parts == before and context.version == version
    assert "/buffer_gain" in take.server.addrs()
    assert "/synth_new" in take.server.addrs()
    assert host.props[wid]["preview_buffer"] >= 0
    take.server.sent.clear()
    editor.preview_off()
    assert "/buffer_alloc" not in take.server.addrs()
    assert host.props[wid]["preview_buffer"] == -1
    editor.preview_on()
    assert "/buffer_alloc" not in take.server.addrs()
    editor.remove_effect()
    assert take.server.addrs().count("/buffer_free") == 2
    assert editor.parts == before and context.version == version
    assert editor.envelope[1] == 0.25


def test_an_empty_sound_file_opens_untitled_and_asks_for_a_path():
    from clausters import SoundFile
    editor, host, _wid, _context = opened(FakeBuffer(4800, 2), file=SoundFile.empty(2, 4800, SR))
    assert editor.buffer.frames == 4800
    file = editor.file
    assert (file.path, file.frames, file.channels) == (None, 4800, 2)
    with pytest.raises(ValueError, match="path"):
        editor.save()
    assert editor.save("/takes/new.wav") == "/takes/new.wav"
    assert editor.file.path == "/takes/new.wav"
    assert editor.file.sample_format == "float"
    # the window numbers the dialogs and holds their stack
    assert "Save as" in str(host.trees[0])


def test_a_new_file_asked_for_in_the_window_is_delivered_to_the_handler():
    from clausters import SoundFile
    editor, _host, _wid, _context = opened(FakeBuffer(), file=SoundFile.empty(1, 100, SR))
    asked = []
    editor._new_file = lambda shape: asked.append(shape)
    editor._take({"turn": "route", "newFile": {"channels": 3, "frames": 10, "rate": SR}})
    assert asked == [{"channels": 3, "frames": 10, "rate": SR}]


def test_a_take_opened_over_a_buffer_is_no_file():
    editor, _host, _wid, _context = opened(FakeBuffer())
    assert editor.file is None


def test_channels_fold_through_the_crate_and_correct_the_take():
    editor, host, wid, _context = opened(FakeBuffer(100, 4))
    editor.fold_channel(1)
    assert editor.folded == [1]
    pushed = host.acks[-1][1]
    assert any("folded" in str(c) and str(wid) in str(c) for c in pushed), pushed
    editor.solo_channel(2)
    assert editor.folded == [0, 1, 3]
    editor.show_all_channels()
    assert editor.folded == []
    with pytest.raises(ValueError, match="channel 7"):
        editor.fold_channel(7)


def test_files_asked_for_in_the_window_are_delivered_to_the_handler():
    editor, _host, _wid, _context = opened(FakeBuffer())
    asked = []
    editor._open_file = lambda path: asked.append(path)
    editor._take({"turn": "route", "openFile": {"paths": ["/takes/b.wav", "/takes/c.wav"]}})
    assert asked == [["/takes/b.wav", "/takes/c.wav"]]


def test_files_share_one_handler_and_transport_but_keep_state_and_history(monkeypatch):
    from clausters import SoundFile
    editor, host, wid, _context = opened(FakeBuffer())
    transport_id = editor._playback.transport_id

    def load(file, *, server=None, **options):
        return types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                     channels=file.channels, sample_rate=file.sample_rate,
                                     server=server, path=file.path)

    monkeypatch.setattr(SoundFile, "to_buffer", load)
    monkeypatch.setattr(SoundFile, "open", staticmethod(lambda path: SoundFile(str(path), 200, 2, SR)))
    editor.set_envelope([0, 1, 1, 0, 100 / SR, 0.5, 1, 0])
    editor._driver.locate(20 / SR)
    editor._driver.set_span((10 / SR, 40 / SR))
    first_envelope = editor.envelope
    listed = lambda: host.trees[-1]["children"][1]["children"][0]["id"]
    files_list = listed()
    editor.open_file(["/takes/b.wav", "/takes/c.wav"])
    assert listed() == files_list, "the list is the window's: a click in flight still finds it"
    assert len(editor.files) == 3
    assert picture(host.trees[-1])["id"] == wid
    assert editor.file.path == "/takes/c.wav"
    assert editor._playback.transport_id == transport_id
    assert editor.can_undo is False
    assert editor._window == 901
    editor.focus_file(0)
    assert editor.cursor == 20 / SR
    assert editor._driver.span == (10 / SR, 40 / SR)
    assert editor.envelope == first_envelope
    assert editor.undo()
    assert editor.can_redo
    editor.focus_file(1)
    assert not editor.can_undo and not editor.can_redo
    editor.focus_file(0)
    assert editor.can_redo
    editor.hide_files()
    assert editor._call("files")["visible"] is False
    editor.show_files()
    assert host.trees[-1]["children"][1]["children"][0]["type"] == "table"
    editor.close_file(2)
    assert len(editor.files) == 2
    assert editor.can_redo
    with pytest.raises(ValueError, match="index"):
        editor.focus_file(-1)
    editor.close_file()
    assert editor.file.path == "/takes/b.wav"
    editor.close_file()
    assert editor.closed
    assert editor.files == ()


def test_playing_a_file_while_viewing_another_keeps_the_signal_and_meter(monkeypatch):
    from clausters import SoundFile
    take = FakeBuffer(channels=1)
    editor, host, _wid, _context = opened(take)
    monkeypatch.setattr(SoundFile, "to_buffer", lambda file, *, server=None, **options:
                        types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                              channels=file.channels, sample_rate=file.sample_rate,
                                              server=server, path=file.path))
    monkeypatch.setattr(SoundFile, "open", staticmethod(
        lambda path: SoundFile(str(path), 200, 6 if "six" in str(path) else 2, SR)))
    first = editor._file_id
    playing_channels = 1
    editor._driver.play()
    take.server.playing = True
    editor.open_file(["/takes/stereo.wav", "/takes/six.wav"])
    assert editor._playback.state()["playing"] == first
    assert editor._playback.state()["meters"]["channels"] == 1
    for index, channels in [(2, 6), (1, 2), (0, 1), (1, 2), (2, 6)]:
        editor.focus_file(index)
        state = editor._playback.state()
        assert state["playing"] == first
        assert state["meters"]["channels"] == playing_channels
        wid = picture(host.trees[-1])["id"]
        assert host.props[wid]["playhead_at"] == (0.0 if editor._file_id == first else -1.0)
        editor._driver.stop()
        take.server.playing = False
        assert editor._playback.state()["meters"] == {"bus": -channels, "channels": channels}
        editor._driver.play()
        take.server.playing = True
        first = editor._file_id
        playing_channels = channels
        assert editor._playback.state()["meters"]["channels"] == channels
    editor.close()


def test_two_audio_windows_keep_distinct_component_bindings_on_one_host():
    take = FakeBuffer()
    first, host, wid, context = opened(take)
    second = AudioEditor(take, context=context)
    second.open(host)
    other_ids = set(second.view.widgets)
    assert set(first.view.widgets).isdisjoint(other_ids)
    first.view_both()
    assert picture(host.trees[-1])["id"] == wid
    assert set(second.view.widgets) == other_ids
    first.close()
    assert not second.closed
    second.close()


def test_reconnecting_refuses_stale_editor_resources_before_sending():
    take = FakeBuffer()
    editor, _host, _wid, _context = opened(take)
    previous_ids = take.server.ids
    sent = len(take.server.sent)
    take.server.ids = _native.IdSpaces(max_nodes=8192, audio_buses=1024, outputs=2,
                                      control_buses=16384, buffers=4096)
    with pytest.raises(RuntimeError, match="previous server instance"):
        _ = editor.parts
    with pytest.raises(RuntimeError, match="previous server instance"):
        editor._playback.call("close")
    assert len(take.server.sent) == sent
    # Keep the original allocator alive until the stale handle is discarded.
    assert previous_ids is not take.server.ids


def test_shared_undo_updates_an_unfocused_file_without_moving_focus(monkeypatch):
    from clausters import SoundFile
    editor, _host, _wid, context = opened(FakeBuffer())
    original = editor.envelope
    editor.set_envelope([0, 0.5, 1, 0, 100 / SR, 0.25, 1, 0])
    monkeypatch.setattr(SoundFile, "to_buffer", lambda file, *, server=None, **options:
                        types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                              channels=file.channels, sample_rate=file.sample_rate,
                                              server=server, path=file.path))
    monkeypatch.setattr(SoundFile, "open", staticmethod(lambda path: SoundFile(str(path), 200, 2, SR)))
    editor.open_file("/takes/b.wav")
    binding = editor._call("binding")
    stepped = context.step("undo")
    assert stepped["stepped"]
    assert stepped["effects"][0]["file"]["id"] != binding["id"]
    context.carry(stepped)
    assert editor._call("binding")["id"] == binding["id"]
    assert editor.file.path == "/takes/b.wav"
    editor.focus_file(0)
    assert editor.envelope == original
    editor.close()


def test_clipboard_replacement_does_not_free_a_previous_server_incarnations_id():
    take = FakeBuffer()
    first, _host, _wid, _context = opened(take)
    first._clipboard(json.dumps({"content": {"kind": "buffer", "channels": 1, "frames": 4, "sample_rate": SR, "buffer": 700}}), 700)
    take.server.ids = _native.IdSpaces(max_nodes=8192, audio_buses=1024, outputs=2,
                                      control_buses=16384, buffers=4096)
    second = AudioEditor(take, context=Editing())
    take.server.sent.clear()
    second._clipboard(json.dumps({"content": {"kind": "buffer", "channels": 1, "frames": 4, "sample_rate": SR, "buffer": 701}}), 701)
    assert not any(addr == "/buffer_free" and args == (700,) for addr, args in take.server.sent)
    second._clipboard(json.dumps({"content": {"kind": "buffer", "channels": 1, "frames": 4, "sample_rate": SR, "buffer": 702}}), 702)
    assert ("/buffer_free", (701,)) in take.server.sent
    second.close()


def test_clipboard_copy_survives_origin_close_and_paste_owns_its_samples():
    take = FakeBuffer()
    first, host, wid, context = opened(take)
    first.apply("/gui_event", [wid, 1, context.version, "copy", 10, 20])
    copied = host.clip
    assert json.loads(copied)["content"]["location"]["publication"] >= 0
    source = json.loads(copied)["content"]["buffer"]
    first.close()
    assert source not in take.server.buffers.freed
    second, other_host, other_wid, other_context = opened(take)
    second.apply("/gui_event", [other_wid, 1, other_context.version, "paste", 50, "buffer", copied])
    assert second.buffer.frames == 120
    assert all(p["source"]["source"] != source for p in second.parts)
    second.apply("/gui_event", [other_wid, 2, other_context.version, "mix", 0, "buffer", copied])
    assert second.buffer.frames == 120
    assert "/buffer_mix" in take.server.addrs()
    second.apply("/gui_event", [other_wid, 3, other_context.version, "copy", 0, 3])
    assert source in take.server.buffers.freed
    assert other_host.clip != copied
    second.undo()
    assert second.buffer.frames == 120
    second.undo()
    assert second.buffer.frames == 100
    second.close()


def test_failed_cut_copy_keeps_contents_and_previous_publication():
    take = FakeBuffer()
    editor, host, wid, context = opened(take)
    editor.apply("/gui_event", [wid, 1, context.version, "copy", 0, 4])
    previous = host.clip
    request = take.server.request

    def refuse_copy(addr, *args, **options):
        if addr == "/buffer_gen":
            return "/fail", [addr, "copy failed"]
        return request(addr, *args, **options)

    take.server.request = refuse_copy
    with pytest.raises(CommandError, match="failed"):
        editor.apply("/gui_event", [wid, 2, context.version, "cut", 10, 20])
    assert editor.buffer.frames == 100
    assert not editor.can_undo
    assert host.clip == previous
    take.server.request = request
    editor.apply("/gui_event", [wid, 3, context.version, "cut", 10, 20])
    assert editor.buffer.frames == 80
    assert host.clip != previous
    editor.close()


def test_paste_refuses_other_servers_stale_publications_and_external_text():
    take = FakeBuffer()
    editor, host, wid, context = opened(take)
    editor.apply("/gui_event", [wid, 1, context.version, "copy", 0, 4])
    copied = host.clip
    other, _host, other_wid, other_context = opened(FakeBuffer())
    other.apply("/gui_event", [other_wid, 1, other_context.version, "paste", 0, "buffer", copied])
    assert other.buffer.frames == 100
    assert not other.can_undo
    editor.apply("/gui_event", [wid, 2, context.version, "copy", 10, 4])
    editor.apply("/gui_event", [wid, 3, context.version, "paste", 0, "buffer", copied])
    assert editor.buffer.frames == 100
    current = host.clip
    editor.apply("/gui_event", [wid, 4, context.version, "paste", 0, "text", json.dumps({"content": {"kind": "text", "text": "external"}})])
    assert json.loads(current)["content"]["buffer"] in take.server.buffers.freed
    assert editor.buffer.frames == 100
    other.close()
    editor.close()


def test_native_editing_handle_serializes_size_and_fill_between_threads(monkeypatch):
    import ctypes
    import concurrent.futures
    import time

    cached = b""

    def call(_handle, data, length, out, cap):
        nonlocal cached
        if out is None:
            cached = json.dumps({"token": json.loads(ctypes.string_at(data, length))["token"]}).encode()
            time.sleep(0.002)
            return len(cached)
        ctypes.memmove(out, cached, min(len(cached), cap))
        return len(cached)

    real_lib = _native.lib()

    class FakeLibrary:
        clausters_apps_editing_new = staticmethod(lambda: 1)
        clausters_apps_editing_free = staticmethod(lambda handle: None if handle.value == 1 else real_lib.clausters_apps_editing_free(handle))
        clausters_apps_editing_call = staticmethod(call)

        def __getattr__(self, name):
            return getattr(real_lib, name)

    fake = FakeLibrary()
    monkeypatch.setattr(_native, "lib", lambda: fake)
    core = _native.EditingCore()
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as workers:
            results = list(workers.map(lambda token: core.call("state", token=token)["token"], range(8)))
        assert results == list(range(8))
    finally:
        core.free()
