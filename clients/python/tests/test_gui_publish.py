"""What actually goes out on the wire when a picture changes
(`clausters.gui.editing.Application.publish`), and what it costs.

**The client holds no picture of the host's.** It used to keep the last tree
per window and send the difference between the two; that is only correct if
that copy equals what the host holds, and it cannot -- the host moves widgets
on its own (a drag writes an offset per frame, a wheel writes a window, a
marquee writes a mark) and screen state is reported by nothing, correctly,
because screen state is the host's. So a publish is a `/gui_def` of a tree, and
the host reconciles it against the only copy that is true.

What is left to check here is therefore not arithmetic but **granularity**,
which is what the change hands to the caller: a `/gui_def` names any widget, so
an edit publishes the one it touched. The measurement at the bottom is what
says that matters, and it is the one `APPLICATION-SCOPE.md`'s AP5 gates on --
the cost of publishing the widget an edit named does not grow with the piece,
and the cost of publishing the window does.
"""

import json

from clausters.gui.editing import Application


WINDOW = 900


def ids_of(tree):
    """Every widget id in a subtree, its root's own included."""
    out = []
    if isinstance(tree, dict):
        if isinstance(tree.get("id"), int):
            out.append(tree["id"])
        for kid in tree.get("children", ()):
            out.extend(ids_of(kid))
    return out


def splice(tree, wid, subtree):
    """`tree` with the widget `wid` replaced by `subtree`, as a redefine does."""
    if isinstance(tree, dict):
        if tree.get("id") == wid:
            return subtree
        kids = tree.get("children")
        if kids:
            return dict(tree, children=[splice(kid, wid, subtree) for kid in kids])
    return tree


class RegistryHost:
    """A host that holds widgets, so naming one it does not have is an error
    here rather than a line in a log."""

    def __init__(self):
        self.trees: dict = {}
        self.defines: list = []
        self.redefines: list = []

    def held(self, window):
        tree = self.trees.get(window)
        return set() if tree is None else set(ids_of(tree)) | {window}

    def define(self, wid, tree, *blobs):
        self.defines.append(wid)
        self.trees[wid] = tree
        return wid

    def redefine(self, wid, subtree, *blobs, window=None):
        window = wid if window is None else window
        assert wid in self.held(window), (
            f"/gui_def {wid}: no such widget -- redefining something this host "
            f"does not hold")
        self.redefines.append(wid)
        self.trees[window] = splice(self.trees[window], wid, subtree)

    # ---- what an application asks of a host and this does not use ----

    def alloc_id(self):
        raise AssertionError("nothing here draws")

    looping = False
    loop = None

    def poll(self, timeout=0.0):
        return None


def an_app(host):
    app = Application()
    app.host = host
    return app


def lane(wid, clips):
    return {"id": wid, "type": "field", "children": clips}


def clip(wid, offset, **props):
    return {"id": wid, "type": "clip", "offset": offset, **props}


def window(*lanes):
    return {"type": "window", "children": list(lanes)}


# ---- what a publish is ----

def test_a_publish_is_a_definition_of_the_widget_it_names():
    host = RegistryHost()
    app = an_app(host)
    was = window(lane(20, [clip(30, 0.0)]), lane(21, [clip(40, 0.0)]))
    app.publish(WINDOW, was)
    assert host.defines == [WINDOW]

    # Nothing is remembered between the two, so the second says as much as the
    # first: what a def costs is the host's answer, and it is the host that
    # knows which widgets kept their identity.
    now = window(lane(20, [clip(30, 2.0)]), lane(21, [clip(40, 4.0), clip(41, 8.0)]))
    app.publish(WINDOW, now)
    assert host.defines == [WINDOW, WINDOW]
    assert host.redefines == []


def test_publishing_the_widget_an_edit_touched_leaves_the_rest_of_the_window():
    # The granularity the caller has, and the reason it is the caller's: an
    # intent names a node and a widget id is derived from it, so the client
    # knows which widget it redrew. The host is what decides what that costs.
    host = RegistryHost()
    app = an_app(host)
    app.publish(WINDOW, window(lane(20, [clip(30, 0.0)]), lane(21, [clip(40, 0.0)])))

    app.publish(21, lane(21, [clip(40, 0.0), clip(41, 8.0)]), window=WINDOW)
    assert host.redefines == [21]
    assert host.defines == [WINDOW], "the window was not redrawn for it"
    # ...and the host's own picture followed, so a later part-publish inside
    # that lane names a widget it holds.
    app.publish(41, clip(41, 12.0), window=WINDOW)
    assert host.redefines == [21, 41]


def test_a_window_republished_after_it_closed_is_no_special_case():
    # It used to be one. A difference against a picture nobody was drawing
    # would have been sets onto freed widgets, so a window that closed had to
    # be forgotten explicitly; nothing is remembered now, so there is nothing
    # to forget and nothing to get wrong.
    host = RegistryHost()
    app = an_app(host)
    app.publish(WINDOW, window(lane(20, [clip(30, 0.0)])))
    host.trees.clear()
    app.publish(WINDOW, window(lane(20, [clip(30, 2.0)])))
    assert host.defines == [WINDOW, WINDOW]


# ---- what a redraw costs, which is what makes the granularity load-bearing ----
#
# The measurement `APPLICATION-SCOPE.md`'s AP5 asks for. What it asserts is the
# **shape** of the answer rather than a byte count, because the shape is the
# load-bearing part: the cost of publishing the widget an edit named does not
# grow with the piece, and the cost of publishing the window does.

class Weigher:
    """A host that weighs what it is told instead of drawing it."""

    def __init__(self):
        self.bytes = 0

    def alloc_id(self):
        return 0

    def define(self, wid, tree, *blobs):
        self.bytes += len(json.dumps(tree))
        return wid

    def redefine(self, wid, tree, *blobs, window=None):
        self.bytes += len(json.dumps(tree))

    def subscribe(self, func):
        return func

    def unsubscribe(self, func):
        pass

    looping = False
    loop = None


def a_multitrack(lanes: int, clips: int, moved: float) -> dict:
    """`lanes` lanes of `clips` clips, with the first clip of the first lane
    dragged to `moved`."""
    out: dict = {"type": "window", "title": "arranger", "children": []}
    wid = 100
    for lane in range(lanes):
        row: dict = {"id": wid, "type": "field", "label": f"track {lane}",
                     "h": 96, "children": []}
        wid += 1
        for clip in range(clips):
            offset = moved if (lane == 0 and clip == 0) else clip * 4.0 * 48_000
            row["children"].append({"id": wid, "type": "field",
                                    "label": f"take {clip}", "offset": offset,
                                    "dur": 4.0 * 48_000})
            wid += 1
        out["children"].append(row)
    return out


def _drag_cost(lanes: int, clips: int, frames: int = 60) -> tuple:
    """What one frame of a drag costs both ways: publishing the widget the edit
    named, and publishing the window it is in."""
    piece = a_multitrack(lanes, clips, 0.0)
    touched = piece["children"][0]["children"][0]

    part = Weigher()
    app = an_app(part)
    for frame in range(1, frames + 1):
        app.publish(touched["id"], dict(touched, offset=frame * 512.0),
                    window=WINDOW)

    whole = Weigher()
    app = an_app(whole)
    for frame in range(1, frames + 1):
        app.publish(WINDOW, a_multitrack(lanes, clips, frame * 512.0))

    return part.bytes / frames, whole.bytes / frames


def test_publishing_what_an_edit_touched_costs_the_same_in_any_size_of_piece():
    # The finding, and the reason the client's picture could go at all:
    # dragging one clip costs the same whether the piece has sixteen clips or
    # six thousand, *if* what is published is the widget the edit named.
    small, _ = _drag_cost(4, 4)
    large, _ = _drag_cost(64, 100)
    assert small == large, "the touched clip's subtree does not grow"
    assert small < 200, "and a drag is one small message a frame"


def test_publishing_the_window_instead_costs_the_whole_piece_every_frame():
    # The other side of it, which is why the granularity is not a refinement:
    # publishing the window is correct and unusable, and it is the caller --
    # not this module -- that has the knowledge to avoid it.
    _, small = _drag_cost(4, 4)
    _, large = _drag_cost(64, 100)
    assert large > 100 * small, "it grows with the piece, and steeply"
    assert large > 500_000, "which at drag rates is megabytes a second of JSON"
