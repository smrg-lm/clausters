"""Nodes (synths and groups) and client-side id allocation.

The server's node tree (`node`): the root group is id 0; clients allocate
positive ids. Add actions match the server: head/tail of a group, before/after
a node, or replace. `Synth` and `Group` are flat handles holding
an id; the `Server` does the OSC.
"""

from enum import IntEnum

ROOT_NODE_ID = 0


class AddAction(IntEnum):
    HEAD = 0
    TAIL = 1
    BEFORE = 2
    AFTER = 3
    REPLACE = 4


class Node:
    def __init__(self, node_id: int):
        self.id = node_id

    def __repr__(self):
        return f"{type(self).__name__}(id={self.id})"


class Synth(Node):
    def __init__(self, node_id: int, defname: str):
        super().__init__(node_id)
        self.defname = defname


class Group(Node):
    pass


class NodeIDAllocator:
    """Hands out node ids from ``start`` (scsynth clients use 1000+)."""

    def __init__(self, start: int = 1000):
        self._next = start
        self._freed: list[int] = []

    def alloc(self) -> int:
        if self._freed:
            return self._freed.pop()
        node_id = self._next
        self._next += 1
        return node_id

    def free(self, node_id: int):
        self._freed.append(node_id)
