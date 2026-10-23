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


@pytest.fixture(autouse=True)
def disk(monkeypatch):
    """What a run does on the disk, recorded rather than done: the renders
    (what one writes is the offline server's, tested where it is,
    `test_render_step`), the moves and the files deleted."""
    done = types.SimpleNamespace(renders=[], moves=[], deleted=[], refuse=None)

    def render(render, progress=None):
        if done.refuse is not None and done.refuse(render["path"]):
            raise OSError("quota exceeded")
        done.renders.append(render)

    monkeypatch.setattr("clausters._steps.render_step", render)
    monkeypatch.setattr("os.replace", lambda a, b: done.moves.append((a, b)))
    monkeypatch.setattr("clausters.gui.editing.audio._delete",
                        lambda files: done.deleted.extend(files or ()))
    return done


def stems(editor) -> list:
    """The take's list as ``(file stem, start, frames)``: which file each part
    reads, and where."""
    import os
    return [(os.path.splitext(os.path.basename(p["path"]))[0], p["start"], p["frames"])
            for p in editor._call("segments")["parts"]]


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

    def verb(self, window, name):
        self.__dict__.setdefault("verbs", []).append(name)

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
    # a folder of no disk: what is done there is recorded (`disk`)
    options.setdefault("workspace", "/work")
    context = Editing()
    editor = AudioEditor(take, context=context, **options)
    host = FakeHost()
    editor.open(host)
    return editor, host, picture(host.trees[0])["id"], context


def test_a_buffer_is_written_to_the_working_folder_and_the_join_reads_it(disk):
    take = FakeBuffer()
    editor, host, _wid, _context = opened(take)
    display = editor.buffer.bufnum
    write = [args for addr, args in take.server.sent if addr == "/buffer_write"][0]
    assert write[0] == take.bufnum and write[1].endswith("-opened.w64"), "the buffer, out"
    assert write[1].startswith("/work/")
    stitched = [args for addr, args in take.server.sent if addr == "/buffer_stitch"]
    assert stitched and stitched[0][0] == display
    assert stitched[0][3] == write[1], "the join reads the file"
    assert picture(host.trees[0])["segments"]["parts"][0]["path"] == write[1]
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


def test_a_stroke_renders_a_file_and_the_join_reads_it(disk):
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    take.server.sent.clear()
    assert editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 40,
                                       [0.5, -0.5], [0.0, 0.0]]) is True
    assert disk.renders[-1]["path"].endswith("-stroke.w64"), "the stroke, rendered"
    assert take.server.addrs() == ["/buffer_stitch"]
    opened_stem = stems(editor)[0][0]
    assert stems(editor) == [(opened_stem, 0, 40), ("1-2-stroke", 0, 2),
                             (opened_stem, 42, 58)]


def test_a_render_the_history_cannot_reach_is_deleted(disk):
    take = FakeBuffer()
    editor, _host, wid, context = opened(take)
    editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]])
    first = disk.renders[-1]["path"]
    assert editor.undo() is True
    assert first not in disk.deleted, "a redo can still find it"
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 30, [0.5], [0.0]])
    assert first in disk.deleted


def test_reverse_renders_the_selection_backwards(disk):
    editor, _host, wid, _context = opened(FakeBuffer())
    editor.apply("/gui_event", [wid, 1, 0, "selection", 10, 20])
    assert editor.reverse() is True
    assert _host.verbs == ["reverse"], "the host performs the window's verb"
    # and the window's event comes back
    editor.apply("/gui_event", [int(editor._window), 2, 0, "reverse"])
    assert disk.renders[-1]["path"].endswith("-reversed.w64")
    assert [frames for _, _, frames in stems(editor)] == [10, 20, 70]
    assert editor.undo() is True
    assert len(stems(editor)) == 1


def test_a_render_s_progress_is_on_the_window_while_it_runs(monkeypatch):
    seen = []
    editor, host, wid, _context = opened(FakeBuffer())
    window = int(editor._window)

    def render(render, progress=None):
        progress(50, 100)
        seen.append(dict(host.props[window]))

    monkeypatch.setattr("clausters._steps.render_step", render)
    editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]])
    assert seen[0]["progress"] == 0.5
    assert seen[0]["progress_label"] == "rendering 1-2-stroke.w64"
    assert host.props[window]["progress"] == -1, "taken away once it has ended"


def test_two_renders_over_two_files_run_at_once_and_land_in_either_order(monkeypatch):
    """**A render does not hold the window**: a stroke in each of two files,
    made on the window's loop, renders on a thread of its own; both are
    rendering at once, the second lands first, and each file's list names
    its own stroke."""
    import threading
    import time
    from clausters import SoundFile
    from clausters.base.loop import EventLoop
    gates, started = {}, []

    def render(render, progress=None):
        if not render["path"]:
            return  # a file's peaks, measured as it opens
        gate = gates.setdefault(render["path"], threading.Event())
        started.append(render["path"])
        assert gate.wait(5), "released"

    monkeypatch.setattr("clausters._steps.render_step", render)
    monkeypatch.setattr(SoundFile, "open", staticmethod(
        lambda path: SoundFile(str(path), 200, 1, SR, codec="wav", sample_format="float")))
    loop = EventLoop("test-renders")
    loop.start()
    try:
        editor, host, wid, context = opened(FakeBuffer())
        host.loop = loop

        def on_loop(func):
            done, out = threading.Event(), []
            loop.post(lambda: (out.append(func()), done.set()))
            assert done.wait(5)
            return out[0]

        def until(check):
            deadline = time.monotonic() + 5
            while not check():
                assert time.monotonic() < deadline, "in time"
                time.sleep(0.005)

        def stroked(index):
            editor.focus_file(index)
            return any(stem.endswith("-stroke") for stem, *_ in stems(editor))

        on_loop(lambda: editor.open_file("/takes/b.wav"))
        until(lambda: on_loop(lambda: len(editor.files)) == 2)
        # a stroke in the second file, then one in the first while it renders
        on_loop(lambda: editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10,
                                                    [0.5], [0.0]]))
        on_loop(lambda: editor.focus_file(0))
        on_loop(lambda: editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 10,
                                                    [0.5], [0.0]]))
        until(lambda: len(started) == 2)
        gates[started[1]].set()
        until(lambda: on_loop(lambda: stroked(0)))
        assert not on_loop(lambda: stroked(1)), "the first is still rendering"
        gates[started[0]].set()
        until(lambda: on_loop(lambda: stroked(1)))
        assert on_loop(lambda: editor.can_undo), "each stroke is its file's edit"
    finally:
        for gate in gates.values():
            gate.set()
        loop.stop()


def test_a_file_changed_on_disk_is_said_when_the_window_comes_back(tmp_path):
    import os
    from clausters.soundfile import SoundFile
    path = str(tmp_path / "take.wav")
    file = SoundFile.write(path, [0.0] * 100, 1, SR, sample_format="float")
    editor, host, _wid, _context = opened(FakeBuffer(), file=file)
    window = int(editor._window)
    editor.apply("/gui_event", [window, 1, 0, "activated", 1])
    assert host.acks[-1][2] is None, "the first look takes the file as it is"
    os.utime(path, (1_000_000, 1_000_000))
    editor.apply("/gui_event", [window, 2, 0, "activated", 1])
    assert host.acks[-1][2] == "take.wav changed on disk"
    editor.apply("/gui_event", [window, 3, 0, "activated", 1])
    assert host.acks[-1][2] is None, "said once"


def test_a_cut_copies_the_span_s_parts_and_renders_nothing(disk):
    take = FakeBuffer()
    editor, host, wid, _context = opened(take)
    take.server.sent.clear()
    rendered = len(disk.renders)
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0])
    assert take.server.addrs()[0] == "/buffer_stitch"
    assert len(disk.renders) == rendered, "nothing is made"
    assert json.loads(host.clip)["content"]["kind"] == "segments"
    assert editor.buffer.frames == 80
    assert editor.undo() is True
    assert editor.buffer.frames == 100


def test_a_delete_removes_samples_without_copying_them():
    take = FakeBuffer()
    editor, host, wid, _context = opened(take)
    take.server.sent.clear()
    editor.apply("/gui_event", [wid, 1, 0, "delete", 10.0, 20.0])
    assert take.server.addrs()[0] == "/buffer_stitch", "no block was copied out"
    assert not hasattr(host, "clip"), "and nothing went on the clipboard"
    assert editor.buffer.frames == 80
    assert editor.undo() is True
    assert editor.buffer.frames == 100


def test_a_save_renders_the_take_over_its_file_or_as_another(disk):
    from clausters.soundfile import SoundFile
    file = SoundFile("/takes/a.wav", 100, 1, SR, "wav", "float")
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take, file=file)
    assert stems(editor) == [("a", 0, 100)], "the file, read where it is"
    assert "/buffer_write" not in take.server.addrs()
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0])
    assert editor.save() == "/takes/a.wav"
    # the file the take reads is moved aside, never written while it is read
    (aside, held), (fresh, back) = disk.moves
    assert aside == "/takes/a.wav" and held.startswith("/work/")
    assert fresh == disk.renders[-1]["path"] and back == "/takes/a.wav"
    assert editor.save("/takes/b.wav", sample_format="int24") == "/takes/b.wav"
    assert (disk.renders[-1]["path"], disk.renders[-1]["format"]) == ("/takes/b.wav", "int24")
    assert len(disk.moves) == 2, "a file nothing reads is rendered in place"
    assert editor.save() == "/takes/b.wav"


def test_a_save_over_a_buffer_reads_a_render_into_it_or_into_a_new_one(disk):
    take = FakeBuffer()
    editor, _host, wid, _context = opened(take)
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0])
    take.server.sent.clear()
    saved = editor.save()
    assert saved.bufnum == take.bufnum and saved.frames == 80
    read = [args for addr, args in take.server.sent if addr == "/buffer_allocRead"]
    assert read[0][0] == take.bufnum and read[0][1] == disk.renders[-1]["path"]
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
    assert taken["channels"] == 2 and "buffer" not in taken
    assert taken["segments"]["parts"][0]["path"].endswith("-opened.w64")
    assert taken["measure"] == "peak rms"
    assert taken["gestures"] == {"drag": "select", "alt": "draw", "ctrl": "sample"}
    assert set(editor.view.props(editor, wid)) == {"segments"}


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
    of the sweep is the span, and release puts the playhead on the span's start
    without moving the position cursor. A pass that runs to its end is left as it was
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
    editor.apply("/gui_event", [wid, 5, 0, "selection", 40, 30, True])
    assert sent("/transport_locateSample")[-1] == [1, 40], "let go: into the span"

    # Not looping, the pass keeps the end it was played with.
    editor.apply("/gui_event", [window, 6, 0, "loop", 0])
    server.sent.clear()
    editor.apply("/gui_event", [wid, 7, 0, "selection", 5, 10])
    editor.apply("/gui_event", [wid, 8, 0, "selection", 5, 10, True])
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


def test_gain_curve_edits_and_apply_share_the_take_history(disk):
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
    assert disk.renders[-1]["path"].endswith("-gain.w64"), "the take, rendered through the curve"
    assert "/buffer_gain" not in take.server.addrs()
    assert [stem.split("-")[-1] for stem, _, _ in stems(editor)] == ["gain"]
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
    editor.free_preview()
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


def _asked(editor) -> list:
    """What the coordinator asks this client to read, instead of reading it."""
    asked = []

    def read(wait):
        asked.append(wait.get("shape") or wait.get("path"))
        raise RuntimeError("not read here")

    editor._shape = read
    return asked


def test_a_new_file_asked_for_in_the_window_is_delivered_to_the_handler():
    from clausters import SoundFile
    editor, _host, _wid, _context = opened(FakeBuffer(), file=SoundFile.empty(1, 100, SR))
    asked = _asked(editor)
    window = int(editor._window)
    editor.apply("/gui_event", [window, 1, 0, "new_file"])
    dialog = lambda name: editor._window_widget("dialog", name)
    editor.apply("/gui_event", [dialog("new:channels"), 2, 0, "3"])
    with pytest.raises(RuntimeError, match="not read here"):
        editor.apply("/gui_event", [dialog("new:ok"), 3, 0, "click"])
    assert len(asked) == 1 and asked[0]["channels"] == 3
    assert set(asked[0]) == {"channels", "frames", "rate"}


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


def test_files_asked_for_are_read_one_at_a_time():
    editor, _host, _wid, _context = opened(FakeBuffer())
    asked = _asked(editor)
    with pytest.raises(RuntimeError, match="not read here"):
        editor.open_file(["/takes/b.wav", "/takes/c.wav"])
    assert asked == ["/takes/b.wav"], "the second is read once the first is added"


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
    assert not editor.closed, "closing the last file leaves the editor open"
    assert editor.files == ()
    assert editor.file is None


def test_playing_a_file_while_viewing_another_silences_the_viewed_meter(monkeypatch):
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
    editor._driver.play()
    take.server.playing = True
    editor.open_file(["/takes/stereo.wav", "/takes/six.wav"])
    assert editor._playback.state()["playing"] == first
    assert editor._playback.state()["meters"] == {"bus": -6, "channels": 6}
    for index, channels in [(2, 6), (1, 2), (0, 1), (1, 2), (2, 6)]:
        editor.focus_file(index)
        state = editor._playback.state()
        assert state["playing"] == first
        assert state["meters"]["channels"] == channels
        assert (state["meters"]["bus"] >= 0) == (editor._file_id == first)
        wid = picture(host.trees[-1])["id"]
        expected = 0.0 if editor._file_id == first else -1.0
        assert host.props[wid]["playhead_at"] == expected
        editor.hide_files()
        def find_pane(node):
            if node.get("id") == editor._window_widget("pane"):
                return node
            return next((found for child in node.get("children", ())
                         if (found := find_pane(child)) is not None), None)
        assert find_pane(host.trees[-1])["axes"]["x"]["playhead_at"] == expected
        editor._driver.stop()
        take.server.playing = False
        assert editor._playback.state()["meters"] == {"bus": -channels, "channels": channels}
        editor._driver.play()
        take.server.playing = True
        first = editor._file_id
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
        editor.close()
    assert len(take.server.sent) == sent
    # Keep the original allocator alive until the stale handle is discarded.
    assert previous_ids is not take.server.ids


def test_the_window_reads_its_actions_from_the_file_in_focus():
    editor, host, wid, context = opened(FakeBuffer())
    window = editor._window
    assert host.trees[0]["actions"]["undo"]["enabled"] is False, "drawn with its actions"
    assert editor.actions["copy"] == {**editor.actions["copy"], "enabled": False,
                                      "reason": "nothing is selected"}
    assert editor.actions["view_wave"]["checked"] is True
    editor.apply("/gui_event", [wid, 1, context.version, "selection", 10, 20])
    assert host.props[window]["actions"]["copy"]["enabled"] is True, "the sweep is read"
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 10, [0.5], [0.0]])
    undo = host.props[window]["actions"]["undo"]
    assert undo["enabled"] is True
    assert undo["target"] == {"kind": "document", "member": editor._member,
                              "object": editor._call("binding")["id"]}
    editor.close()


def test_a_second_editor_never_walks_the_first_one_s_edits():
    take = FakeBuffer()
    first, _host, wid, context = opened(take)
    first.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]])
    before = first.parts
    second = AudioEditor(FakeBuffer(), context=context)
    second.open(FakeHost())
    assert second.can_undo is False and second.actions["undo"]["enabled"] is False
    assert second.undo() is False
    assert first.parts == before and first.can_undo is True
    first.close()
    second.close()


def test_a_shared_undo_from_one_window_tells_it_the_version_the_step_s_commit_left():
    class Told(FakeHost):
        def __init__(self):
            super().__init__()
            self.versions: list = []

        def ack(self, seq, doc_version=0, reason=None):
            self.versions.append(doc_version)
            super().ack(seq, doc_version, reason)

        def push(self, seq, *corrections, doc_version=0, reason=None):
            self.versions.append(doc_version)
            super().push(seq, *corrections, doc_version=doc_version, reason=reason)

    context = Editing()
    take, other = FakeBuffer(), FakeBuffer()
    other.bufnum, other.server = 8, take.server
    first = AudioEditor(take, context=context, shared_history=True)
    second = AudioEditor(other, context=context, shared_history=True)
    one, two = Told(), Told()
    first.open(one)
    second.open(two)
    wid = picture(two.trees[-1])["id"]
    second.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]])
    assert first.undo()
    assert one.versions[-1] == context.version
    first.close()
    second.close()


def test_shared_undo_updates_an_unfocused_file_without_moving_focus(monkeypatch):
    from clausters import SoundFile
    editor, _host, _wid, context = opened(FakeBuffer(), shared_history=True)
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


def test_a_copy_outlives_the_editor_it_was_made_in_and_pastes_elsewhere(disk):
    take = FakeBuffer()
    first, host, wid, context = opened(take)
    first.apply("/gui_event", [wid, 1, context.version, "copy", 10, 20])
    copied = host.clip
    named = json.loads(copied)["content"]["parts"][0]["path"]
    assert named.startswith("/work/")
    first.close()
    assert named not in disk.deleted, "the clipboard still names it"
    second, other_host, other_wid, other_context = opened(FakeBuffer())
    second.apply("/gui_event", [other_wid, 1, other_context.version, "paste", 50, "segments", copied])
    assert second.buffer.frames == 120
    assert [p["path"] for p in second._call("segments")["parts"]][1] == named
    second.apply("/gui_event", [other_wid, 2, other_context.version, "mix", 0, "segments", copied])
    assert second.buffer.frames == 120
    assert disk.renders[-1]["path"].endswith("-mixed.w64")
    second.apply("/gui_event", [other_wid, 3, other_context.version, "copy", 0, 3])
    assert other_host.clip != copied
    second.undo()
    assert second.buffer.frames == 120
    second.undo()
    assert second.buffer.frames == 100
    second.close()


def test_a_failed_cut_keeps_the_take():
    take = FakeBuffer()
    editor, host, wid, context = opened(take)
    request = take.server.request

    def refuse_stitch(addr, *args, **options):
        if addr == "/buffer_stitch":
            return "/fail", [addr, "stitch failed"]
        return request(addr, *args, **options)

    take.server.request = refuse_stitch
    with pytest.raises(CommandError, match="failed"):
        editor.apply("/gui_event", [wid, 2, context.version, "cut", 10, 20])
    assert editor.buffer.frames == 100
    assert not editor.can_undo
    take.server.request = request
    # the join over what was there is stitched again before anything else
    editor.retry_pending()
    editor.apply("/gui_event", [wid, 3, context.version, "cut", 10, 20])
    assert editor.buffer.frames == 80
    editor.close()


def test_a_paste_of_text_puts_nothing_in_and_clears_the_copy(disk):
    take = FakeBuffer()
    editor, host, wid, context = opened(take)
    editor.apply("/gui_event", [wid, 1, context.version, "copy", 0, 4])
    copied = host.clip
    editor.apply("/gui_event", [wid, 2, context.version, "paste", 0, "text",
                                json.dumps({"content": {"kind": "text", "text": "external"}})])
    assert editor.buffer.frames == 100
    assert not editor.can_undo
    editor.apply("/gui_event", [wid, 3, context.version, "paste", 0, "segments", copied])
    assert editor.buffer.frames == 104, "a copy pasted again is still the copy"
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


def test_undo_redo_correct_only_composed_views_while_optional_ids_stay_stable():
    editor, host, wid, context = opened(FakeBuffer())

    def tree_ids(node):
        result = {node["id"]} if "id" in node else set()
        for child in node.get("children", ()):
            result.update(tree_ids(child))
        return result

    editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]])
    lane = editor._window_widget("lane")
    for configure in (editor.view_wave, editor.view_both, editor.view_spectrogram,
                      editor.view_both, editor.view_wave):
        configure()
        present = tree_ids(host.trees[-1])
        assert set(editor.view.widgets) <= present
        assert (lane in editor.view.widgets) == (lane in present)
        assert editor._window_widget("lane") == lane
        for walk in (editor.undo, editor.redo):
            host.acks.clear()
            assert walk()
            corrections = [c for ack in host.acks for c in ack[1]]
            assert corrections
            assert all(widget in present for widget, _props in corrections)
            assert any(widget == wid for widget, _props in corrections)
    editor.close()


def test_failed_save_keeps_destination_and_undo_to_saved_revision_is_clean(disk):
    take = FakeBuffer()
    editor, _host, wid, context = opened(take)
    editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]])
    editor.save("/takes/saved.wav")
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 20, [0.25], [0.0]])
    disk.refuse = lambda path: path == "/takes/refused.wav"
    with pytest.raises(CommandError, match="quota"):
        editor.save("/takes/refused.wav")
    assert editor.file.path == "/takes/saved.wav"
    assert editor.dirty
    disk.refuse = None
    assert editor.undo()
    assert not editor.dirty
    assert editor.redo()
    assert editor.dirty
    assert editor.save() == "/takes/saved.wav"
    assert not editor.dirty
    editor.close()


def test_a_new_file_whose_render_fails_is_not_added_and_its_buffer_returns(disk):
    take = FakeBuffer()
    editor, _host, _wid, _context = opened(take)
    disk.refuse = lambda path: path.endswith("-new.w64")
    before = len(editor.files)
    with pytest.raises(CommandError, match="quota"):
        editor.new_file(1, 50, SR)
    assert len(editor.files) == before
    assert len(take.server.buffers.freed) == len(set(take.server.buffers.freed))
    disk.refuse = None
    editor.new_file(1, 50, SR)
    assert len(editor.files) == before + 1
    editor.close()


def test_failed_repair_is_discoverable_and_retryable_without_repeating_the_edit():
    take = FakeBuffer()
    editor, host, wid, context = opened(take)
    before = editor.parts
    version = context.version
    request = take.server.request
    take.server.request = lambda addr, *args, **kw: (
        ("/fail", [addr, "temporary failure"]) if addr == "/buffer_stitch" else request(addr, *args, **kw))
    with pytest.raises(CommandError, match="recovery is pending"):
        editor.apply("/gui_event", [wid, 1, version, "draw", 0, 10, [0.5], [0.0]])
    pending = context._service_call("operation", action="pendingRepairs", member=editor._member)["repairs"]
    assert len(pending) == 1 and not pending[0]["running"]
    with pytest.raises(CommandError, match="recovery is pending"):
        editor.retry_pending()
    assert editor.parts == before and context.version == version
    echoes = len(host.acks)
    take.server.request = request
    editor.retry_pending()
    assert len(host.acks) == echoes, "a later repair must not repeat the gesture receipt"
    assert editor.parts == before and context.version == version
    assert not editor.can_undo
    assert context._service_call("operation", action="pendingRepairs", member=editor._member)["repairs"] == []
    freed = list(take.server.buffers.freed)
    editor.retry_pending()
    assert take.server.buffers.freed == freed
    assert len(freed) == len(set(freed))
    assert editor.apply("/gui_event", [wid, 2, version, "draw", 0, 10, [0.5], [0.0]])
    assert editor.save("/takes/recovered.wav") == "/takes/recovered.wav"
    editor.close_file()


def test_envelope_visibility_changed_during_an_edit_survives_confirmation_and_history():
    take = FakeBuffer()
    editor, _host, wid, context = opened(take)
    request = take.server.request
    shown = False

    def delayed(addr, *args, **kwargs):
        nonlocal shown
        if addr == "/buffer_stitch" and not shown:
            shown = True
            editor.show_envelope()
        return request(addr, *args, **kwargs)

    def pane(node):
        if node.get("type") == "pane":
            return node
        return next((found for child in node.get("children", ()) if (found := pane(child)) is not None), None)

    take.server.request = delayed
    assert editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]])
    for step in [None, editor.undo, editor.redo]:
        if step is not None:
            assert step()
        assert pane(editor.draw())["hidden"] == ""
        assert editor.layer == "points"
    editor.close()


def test_an_editor_opens_with_no_file_and_its_first_file_comes_into_focus(monkeypatch):
    from clausters import SoundFile
    server = FakeServer()
    editor = AudioEditor(server=server, context=Editing())
    host = FakeHost()
    editor.open(host)
    tree = host.trees[-1]
    assert picture(tree)["type"] == "label", "where the take would be, how to open one"
    assert "menu" in tree, "the window keeps its chrome"
    assert editor.file is None and editor.files == ()
    with pytest.raises(ValueError, match="no file is open"):
        editor.parts
    with pytest.raises(ValueError, match="no file is open"):
        editor.save("/takes/x.wav")
    monkeypatch.setattr(SoundFile, "to_buffer", lambda file, *, server=None, **options:
                        types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                              channels=file.channels, sample_rate=file.sample_rate,
                                              server=server, path=file.path))
    monkeypatch.setattr(SoundFile, "open", staticmethod(lambda path: SoundFile(str(path), 200, 2, SR)))
    editor.open_file("/takes/b.wav")
    assert editor.file.path == "/takes/b.wav"
    assert picture(host.trees[-1])["type"] == "signal", "the file is drawn"
    assert len(editor.parts) == 1
    editor.close_file()
    assert not editor.closed
    assert editor.file is None
    assert picture(host.trees[-1])["type"] == "label", "drawn again, empty"


def test_closing_a_file_keeps_the_meter_and_corrects_only_what_is_drawn(monkeypatch):
    from clausters import SoundFile

    def nodes(node):
        yield node
        for child in node.get("children", ()):
            yield from nodes(child)

    class Watch(FakeHost):
        def __init__(self):
            super().__init__()
            self.absent: list = []

        def set(self, id, **props):
            # the window itself is the host's, everything else is drawn
            if id < 900 and id not in {n.get("id") for n in nodes(self.trees[-1])}:
                self.absent.append((id, props))
            super().set(id, **props)

    def meters(tree):
        return [n for n in nodes(tree) if n.get("type") == "meter"]

    monkeypatch.setattr(SoundFile, "to_buffer", lambda file, *, server=None, **options:
                        types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                              channels=file.channels, sample_rate=file.sample_rate,
                                              server=server, path=file.path))
    monkeypatch.setattr(SoundFile, "open", staticmethod(lambda path: SoundFile(str(path), 200, 2, SR)))
    editor = AudioEditor(FakeBuffer(), context=Editing())
    host = Watch()
    editor.open(host)
    editor.open_file(["/takes/a.wav", "/takes/b.wav"])
    assert meters(host.trees[-1])
    editor.close_file()
    assert meters(host.trees[-1]), "the meter follows the file viewed now"
    editor.close_file()
    editor.close_file()
    assert not meters(host.trees[-1]), "an empty window draws no meter"
    editor.open_file("/takes/c.wav")
    assert meters(host.trees[-1])
    assert host.absent == [], "nothing addressed to a widget the window does not draw"


#: The shared trace both clients replay: the same actions, in the same order,
#: must come back as the same effects and awaits from the coordinator.
TRACE_VECTORS = __import__("pathlib").Path(__file__).resolve().parents[2] / "web" / "tests" / "audio-trace-vectors.json"


def audio_trace(monkeypatch) -> list:
    """**One script of actions over an audio editor**, and what each one came
    to: the effect and await kinds the client carried out, in order. The web
    client's twin runs the same script (`gui-audio.test.ts`)."""
    from clausters import SoundFile
    monkeypatch.setattr(SoundFile, "to_buffer", lambda file, *, server=None, **options:
                        types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                              channels=file.channels, sample_rate=file.sample_rate,
                                              server=server, path=file.path))
    monkeypatch.setattr(SoundFile, "open", staticmethod(lambda path: SoundFile(str(path), 200, 2, SR)))
    # what a render writes is the offline server's, not this script's
    monkeypatch.setattr("clausters._steps.render_step", lambda render, progress=None: None)
    take = FakeBuffer(100, 1)
    trace: list = []
    editor = AudioEditor(take, context=Editing())
    effect, wait = editor._effect, editor._await

    def record_effect(entry, held):
        trace[-1][1].append(entry["kind"])
        return effect(entry, held)

    def record_wait(entry):
        trace[-1][1].append(f"await:{entry['kind']}" + (f":{entry['via']}" if "via" in entry else ""))
        return wait(entry)

    editor._effect, editor._await = record_effect, record_wait
    host = FakeHost()

    def step(name, act):
        trace.append([name, []])
        act()

    step("open", lambda: editor.open(host))
    window = int(editor._window)
    step("space bar, stopped", lambda: editor.apply("/gui_event", [window, 1, 0, "play", 0]))
    take.server.playing = True
    step("space bar, rolling", lambda: editor.apply("/gui_event", [window, 2, 0, "play", 0]))
    take.server.playing = False
    step("span", lambda: editor._driver.set_span((10 / SR, 40 / SR)))
    step("loop", lambda: editor._driver.set_looping(True))
    wid = picture(host.trees[-1])["id"]
    step("cut", lambda: editor.apply("/gui_event", [wid, 3, editor._editing.version, "cut", 10.0, 20.0]))
    step("undo from the window", lambda: editor.apply("/gui_event", [window, 4, 0, "undo"]))
    step("save", lambda: editor.save("/takes/a.wav"))
    step("open a file", lambda: editor.open_file(["/takes/b.wav"]))
    step("focus the first", lambda: editor.focus_file(0))
    step("close the focused", lambda: editor.close_file())
    step("both views", lambda: editor.view_both())
    step("close", lambda: editor.close())
    return trace


def test_the_shared_trace_is_what_the_coordinator_answers_this_client(monkeypatch):
    expected = json.loads(TRACE_VECTORS.read_text())["trace"]
    assert audio_trace(monkeypatch) == expected, \
        "regenerate with clients/web/tests/gen-audio-trace-vectors.py if the coordinator moved"


def _node(node: dict, wanted):
    """The first node of a tree ``wanted`` says yes to."""
    if wanted(node):
        return node
    for child in node.get("children", ()):
        found = _node(child, wanted)
        if found is not None:
            return found
    return None


class FaultyServer(FakeServer):
    """**A server that refuses, and one that is slow**: each command named in
    ``fail`` is answered once with a ``/fail``, and while the first command
    named in ``slow`` is out, its action runs before the reply comes -- what
    a hand does in the window while a write is in flight. ``"render"`` names
    an offline render, which `render` carries out."""

    def render(self, render, progress=None):
        if "render" in self.fail:
            raise OSError(self.fail.pop("render"))
        if "render" in self.slow:
            self.slow.pop("render")()

    def __init__(self):
        super().__init__()
        self.fail: dict = {}
        self.slow: dict = {}

    def request(self, addr, *args, expect=None, timeout=None):
        if addr in self.fail:
            self.send_msg(addr, *args)
            return "/fail", [addr, self.fail.pop(addr)]
        if addr in self.slow:
            meanwhile = self.slow.pop(addr)
            self.send_msg(addr, *args)
            meanwhile()
            return "/done", [addr, int(args[0])]
        return super().request(addr, *args, expect=expect, timeout=timeout)


def audio_fault_trace(monkeypatch) -> list:
    """**One script of failures and delays over an audio editor**, and what
    each step left: ``[name, kinds, state]``. ``kinds`` are the effect and
    await kinds in order, or ``None`` for a step where something happens
    while a reply is out -- there the order is the platform's scheduling
    (this client runs the other action inside the wait, a page queues it
    behind it) and only what is left is the contract. The web client's twin
    runs the same script (`gui-audio.test.ts`)."""
    from clausters import SoundFile
    monkeypatch.setattr(SoundFile, "to_buffer", lambda file, *, server=None, **options:
                        types.SimpleNamespace(bufnum=server.buffers.alloc(), frames=file.frames,
                                              channels=file.channels, sample_rate=file.sample_rate,
                                              server=server, path=file.path))
    monkeypatch.setattr(SoundFile, "open", staticmethod(lambda path: SoundFile(str(path), 200, 2, SR)))
    take = FakeBuffer(100, 1)
    take.path = "/takes/a.wav"
    server = take.server = FaultyServer()
    monkeypatch.setattr("clausters._steps.render_step", server.render)
    trace: list = []
    editor = AudioEditor(take, context=Editing(), file=SoundFile("/takes/a.wav", 100, 1, SR))
    effect, wait = editor._effect, editor._await

    def record_effect(entry, held):
        if trace[-1][1] is not None:
            trace[-1][1].append(entry["kind"])
        return effect(entry, held)

    def record_wait(entry):
        if trace[-1][1] is not None:
            trace[-1][1].append(f"await:{entry['kind']}" + (f":{entry['via']}" if "via" in entry else ""))
        return wait(entry)

    editor._effect, editor._await = record_effect, record_wait
    host = FakeHost()

    def state():
        file = editor.file
        return {"path": None if file is None else file.path,
                "frames": None if file is None else file.frames,
                "dirty": editor.dirty, "files": len(editor.files),
                "undo": editor.can_undo, "closed": editor.closed}

    def step(name, act, *, concurrent=False):
        trace.append([name, None if concurrent else [], None])
        try:
            act()
            raised = None
        except CommandError:
            raised = "command"
        except ValueError:
            raised = "value"
        except RuntimeError:
            raised = "runtime"
        trace[-1][2] = {**state(), "raised": raised}

    def cut(at=10.0):
        wid = picture(host.trees[-1])["id"]
        editor.apply("/gui_event", [wid, len(trace), editor._editing.version, "cut", at, 20.0])

    def window(verb):
        editor.apply("/gui_event", [int(editor._window), len(trace), 0, verb])

    def pick(index):
        """A file chosen in the window's list, by the hand."""
        listed = _node(host.trees[-1], lambda node: "rows" in node)
        editor.apply("/gui_event", [listed["id"], len(trace), 0, "select", index])

    def slow(addr, meanwhile, act):
        server.slow[addr] = meanwhile
        act()

    step("open", lambda: editor.open(host))
    step("cut", cut)
    server.fail["render"] = "no space left on device"
    step("a refused save", lambda: editor.save())
    step("the save retried", lambda: editor.save())
    step("a cut while the write is out", lambda: slow("render", cut, editor.save),
         concurrent=True)
    step("undo to what was written", lambda: window("undo"))
    server.fail["/buffer_stitch"] = "stitch refused"
    step("a cut whose stitch is refused", cut)
    step("open another file", lambda: editor.open_file(["/takes/b.wav"]))
    step("back to the first", lambda: editor.focus_file(0))
    step("the second cut while the first is written",
         lambda: slow("render", lambda: (pick(1), cut()), editor.save),
         concurrent=True)
    step("back to the first again", lambda: editor.focus_file(0))
    step("the window closed while a write is out",
         lambda: slow("render", editor.close, lambda: editor.save("/takes/c.wav")),
         concurrent=True)
    return trace


def test_the_shared_fault_trace_is_what_the_coordinator_answers_this_client(monkeypatch):
    trace = audio_fault_trace(monkeypatch)
    states = {name: state for name, _, state in trace}
    # the contract each step keeps, read here as well as frozen
    assert states["a refused save"]["raised"] == "command"
    assert states["a refused save"]["dirty"] is True
    assert states["the save retried"]["dirty"] is False
    assert states["a cut while the write is out"]["dirty"] is True
    assert states["undo to what was written"]["dirty"] is False
    assert states["a cut whose stitch is refused"]["frames"] == states["undo to what was written"]["frames"]
    second = states["the second cut while the first is written"]
    assert second["path"] == "/takes/b.wav" and second["dirty"] is True
    assert states["back to the first again"]["dirty"] is False
    closed = states["the window closed while a write is out"]
    assert closed["closed"] is True and closed["dirty"] is False and closed["path"] == "/takes/c.wav"
    expected = json.loads(TRACE_VECTORS.read_text())["faults"]
    assert trace == expected, \
        "regenerate with clients/web/tests/gen-audio-trace-vectors.py if the coordinator moved"
