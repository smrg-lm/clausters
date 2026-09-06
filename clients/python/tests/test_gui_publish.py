"""What actually goes out on the wire when a picture changes
(`clausters.gui.editing.Application.publish`).

`clausters-core`'s own tests check the **difference** -- that no set names a
widget under a redefine beside it. This is the other end of the same question,
and it exists because the defect was seen there rather than in the arithmetic:
a host warned ``/gui_set 1005: no such widget`` immediately after a publish
that redefined the widget 1005 was inside.

So the host double here is not a recorder. It keeps a **widget registry** the
way a host does -- a `/gui_def` of a subtree frees every id under it and builds
the new one, a `/gui_set` addresses a number that must still be there -- and it
raises where the real host only warns into a log nobody reads.

What a pass means. It says the publish loop is sound **while the client's
picture and the host's agree**, which is what this double makes true by
construction. It does not exonerate the running program, where the host also
moves widgets on its own and is the only one who knows: that is the question
`APPLICATION-SCOPE.md` records as "A set is addressed to a widget the redefine
beside it just removed", and it is settled by the two halves together -- this
one passing and the one on the wire still warning is evidence that the pictures
diverged, not that the arithmetic is wrong.
"""

import random

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
    """A host that holds widgets, so addressing a freed one is an error here
    too rather than a line in a log."""

    def __init__(self):
        self.trees: dict = {}
        self.defines: list = []
        self.redefines: list = []
        self.sets: list = []

    # ---- the registry ----

    def held(self, window):
        tree = self.trees.get(window)
        return set() if tree is None else set(ids_of(tree)) | {window}

    # ---- the three messages ----

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

    def set(self, wid, **props):
        assert wid in self.held(WINDOW), (
            f"/gui_set {wid}: no such widget -- the set rides on a number a "
            f"redefine in the same publish already freed")
        self.sets.append((wid, props))

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


# ---- the shape the log showed ----

def test_a_lane_that_grew_and_a_clip_that_moved_go_out_in_one_publish():
    # One redefine and two sets, which is the publish the warning came out of:
    # the lane 21 grows a clip while 30 -- in the *other* lane -- moves.
    host = RegistryHost()
    app = an_app(host)
    was = window(lane(20, [clip(30, 0.0)]), lane(21, [clip(40, 0.0)]))
    app.publish(WINDOW, was)
    assert host.defines == [WINDOW]

    now = window(lane(20, [clip(30, 2.0)]), lane(21, [clip(40, 4.0), clip(41, 8.0)]))
    assert app.publish(WINDOW, now) is True
    assert host.redefines == [21]
    # 40 moved too, and it rides inside the redefine rather than beside it.
    assert [wid for wid, _ in host.sets] == [30]


def test_a_widget_that_left_the_picture_is_never_set():
    host = RegistryHost()
    app = an_app(host)
    app.publish(WINDOW, window(lane(20, [clip(30, 0.0), clip(31, 4.0)])))
    # 31 goes and 30 moves in the same turn: the lane is rebuilt and carries
    # both, so nothing is addressed to either number.
    app.publish(WINDOW, window(lane(20, [clip(30, 1.0)])))
    assert host.redefines == [20]
    assert host.sets == []


def test_a_window_republished_after_it_closed_is_defined_whole():
    # The registry's other half: a set to a window nobody is drawing is the
    # same defect one size up, and `forget_window` is what stops it.
    host = RegistryHost()
    app = an_app(host)
    app.publish(WINDOW, window(lane(20, [clip(30, 0.0)])))
    app.forget_window(WINDOW)
    host.trees.clear()
    app.publish(WINDOW, window(lane(20, [clip(30, 2.0)])))
    assert host.defines == [WINDOW, WINDOW], "whole, not a set onto nothing"


# ---- and the same thing over pictures nobody chose ----

def a_picture(rng, counter):
    """A window of lanes of clips, ids from one counter -- the host's
    namespace, as the ids milestone made it."""
    lanes = []
    for _ in range(rng.randrange(4)):
        clips = []
        for _ in range(rng.randrange(4)):
            wid = next(counter)
            clips.append(clip(wid, rng.randrange(80) / 8.0,
                              **({"gain": rng.randrange(80) / 8.0}
                                 if rng.random() < 0.3 else {})))
        lanes.append(lane(next(counter), clips))
    return window(*lanes)


def edited(rng, tree, counter):
    """The next picture: one of the changes a hand makes, at every level."""
    out = dict(tree)
    roll = rng.randrange(12)
    if roll == 0:
        out["offset"] = rng.randrange(80) / 8.0
    elif roll == 1:
        out.pop("gain", None)
    elif roll == 2 and "id" in out:
        out["type"] = "knob"
    elif roll == 3 and "id" in out:
        out["id"] = next(counter)
    elif roll == 4:
        out["children"] = list(out.get("children", ())) + [clip(next(counter), 0.0)]
    elif roll == 5 and out.get("children"):
        out["children"] = list(out["children"])[:-1]
    if out.get("children"):
        out["children"] = [edited(rng, kid, counter) for kid in out["children"]]
    return out


def test_no_publish_ever_addresses_a_widget_the_host_no_longer_holds():
    # The assertion is the host's own: every `set` and every `redefine` above
    # checks the registry, so this only has to keep publishing -- and count,
    # because a run that never sent a redefine and a set in one publish would
    # pass without having asked anything.
    seen = {"whole": 0, "redefine": 0, "set": 0, "both": 0}
    for seed in range(60):
        rng = random.Random(seed)
        counter = iter(range(10, 10_000))
        host = RegistryHost()
        app = an_app(host)
        tree = a_picture(rng, counter)
        app.publish(WINDOW, tree)
        for _ in range(20):
            before = (len(host.defines), len(host.redefines), len(host.sets))
            tree = edited(rng, tree, counter)
            app.publish(WINDOW, tree)
            grew = [len(host.defines), len(host.redefines), len(host.sets)]
            whole, redefine, sets = (now > was for now, was in zip(grew, before))
            seen["whole"] += whole
            seen["redefine"] += redefine
            seen["set"] += sets
            seen["both"] += redefine and sets
    assert all(count > 50 for count in seen.values()), \
        f"the publishes generated are not varied enough to mean anything: {seen}"
