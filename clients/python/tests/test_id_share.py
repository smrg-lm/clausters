"""Two clients on one server: the id spaces they split, and the arithmetic
that keeps them apart.

The client range is one range, and every client allocates from it — exact
while a server has one client, a fiction the moment it has two -- two
processes driving one server, a script authoring beside a page on the same
engine.

What makes it work without a negotiation is that there is nothing to
negotiate: equal slices in a fixed order, so ``IdShare(0, 2)`` and
``IdShare(1, 2)`` are disjoint by construction. The web client's
``tests/share.test.ts`` is this suite's counterpart, assertion for assertion.
"""

import pytest

from clausters import _native
from clausters.base import IdShare, WHOLE_SHARE, share_of
from clausters.defs import Server, ServerOptions
from clausters.gui import GuiHost
from clausters.gui.ids import BASE_ID, CAPACITY, GuiIdAllocator


def test_the_default_share_is_the_whole_space():
    part = _native.node_id_partition(8192)
    assert share_of(part["client_base"], part["client_capacity"]) == (
        part["client_base"], part["client_capacity"])
    assert share_of(0, 100, WHOLE_SHARE) == (0, 100)


def test_shares_tile_the_range_exactly():
    # No id belongs to two clients, and none belongs to nobody: a range that
    # does not divide evenly must not leave a gap at the top.
    slices = [share_of(1000, 10_001, IdShare(i, 3)) for i in range(3)]
    assert [base for base, _ in slices] == [1000, 4333, 7666]
    nxt = 1000
    for base, span in slices:
        assert base == nxt          # a slice starts where the previous ends
        nxt = base + span
    assert nxt == 11_001            # and they cover the whole range


def test_a_share_outside_its_split_is_refused():
    with pytest.raises(ValueError):
        IdShare(2, 2)
    with pytest.raises(ValueError):
        IdShare(-1, 2)
    with pytest.raises(ValueError):
        IdShare(0, 0)


def test_two_clients_of_one_server_cannot_collide():
    kernel = Server(share=IdShare(0, 2))
    page = Server(share=IdShare(1, 2))

    # Every space a client allocates from, not the node ids alone.
    nodes = (kernel.nodes.alloc(), page.nodes.alloc())
    assert nodes[0] != nodes[1] and nodes[0] < nodes[1]
    assert kernel.buffers.alloc() != page.buffers.alloc()
    assert kernel.audio_buses.alloc(2).index != page.audio_buses.alloc(2).index
    assert kernel.control_buses.alloc().index != page.control_buses.alloc().index

    # The second share starts past the whole first one, so exhausting one
    # client's range never walks into the other's.
    part = _native.node_id_partition(ServerOptions().max_nodes)
    _, span = share_of(part["client_base"], part["client_capacity"], IdShare(0, 2))
    assert nodes[1] >= part["client_base"] + span

    kernel.close()
    page.close()


def test_a_shared_client_keeps_the_servers_reservations():
    # The output buses are the server's, not a client's, so neither share may
    # hand them out — a split must not open a hole below itself.
    page = Server(share=IdShare(1, 2))
    whole = Server()
    assert page.audio_buses.alloc(2).index >= 2
    assert whole.audio_buses.alloc(2).index >= 2
    page.close()
    whole.close()


def test_widget_ids_split_the_same_way():
    kernel = GuiIdAllocator(share=IdShare(0, 2))
    page = GuiIdAllocator(share=IdShare(1, 2))
    assert kernel.alloc() == BASE_ID
    assert page.alloc() == BASE_ID + CAPACITY // 2


def test_a_host_client_takes_the_share_it_is_given():
    kernel = GuiHost(share=IdShare(0, 2))
    page = GuiHost(share=IdShare(1, 2))
    assert kernel.alloc_id() != page.alloc_id()
