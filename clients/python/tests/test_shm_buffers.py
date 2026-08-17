"""The **material** in a shared segment: what a local client maps rather than asks for.

A `clausters --shm <path>` server puts every pool buffer's samples in a region
beside the segment and indexes them in its directory (ABI v9), so a client on
the same machine reads and writes the take itself instead of fetching it. These
check the client's half of that: the layout it mirrors, the directory it reads,
and the mapping it hands back.

They run against a **hand-built** segment rather than a live server, because
what is under test is the mirror: three readers pin one layout by hand (this
client, the GUI host, and the server that writes it), and what keeps them
honest is that the numbers are asserted on each side.
"""

import struct

import pytest

from clausters.ipc import SEGMENT_SIZE, ShmClient


def test_the_segment_size_is_the_one_the_server_builds():
    """Pinned on both sides: `tests/ipc.rs` asserts the same number in Rust.

    It is documentation, and documentation that is wrong is worse than none —
    this constant said 1024 control buses until 2026-08-17, against a server
    that has had 16384 for a long time, and nothing caught it because a client
    maps the file's own length.
    """
    assert SEGMENT_SIZE == 820_928


def _segment(tmp_path, control_buses=4, taps=1, tap_frames=64, buffers=4):
    """A segment as the server writes one, small enough to read by hand."""
    from clausters.ipc import (_MAGIC, ABI_VERSION, _BUFFER_ROW, _RING_CAPACITY,
                               _RING_HEADER, _TAP_ALIGN, _bus_region_offset,
                               _tap_region_offset)

    tap_at = _tap_region_offset(control_buses)
    size = tap_at + taps * (_TAP_ALIGN + 4 * tap_frames) + buffers * _BUFFER_ROW
    raw = bytearray(size)
    struct.pack_into("<II", raw, 0, _MAGIC, ABI_VERSION)
    struct.pack_into("<I", raw, 28, control_buses)
    struct.pack_into("<II", raw, 32, taps, tap_frames)
    struct.pack_into("<I", raw, 40, 128)  # audio buses
    path = tmp_path / "seg"
    path.write_bytes(bytes(raw))
    assert _bus_region_offset(control_buses) < tap_at
    return path


def test_an_empty_slot_reads_as_no_buffer(tmp_path):
    client = ShmClient(str(_segment(tmp_path)))
    assert client.buffers == 4, "the directory is the segment's tail"
    assert client.buffer_info(0) is None, "a zero generation is an empty slot"
    assert client.map_buffer(0) is None
    assert client.buffer_info(99) is None, "and a row that does not exist"


def test_a_published_buffer_is_read_and_mapped(tmp_path):
    """The whole point: the samples are the server's memory, not a copy of it."""
    path = _segment(tmp_path)
    client = ShmClient(str(path))
    at = client._buffers_at

    # What the server writes when it installs a buffer: an **odd** generation,
    # then the shape.
    struct.pack_into("<Q", client.mm, at, 3)
    struct.pack_into("<II", client.mm, at + 8, 8, 2)
    struct.pack_into("<d", client.mm, at + 16, 48_000.0)
    assert client.buffer_info(0) == (3, 8, 2, 48_000.0)

    # ...and the region beside it, named from the segment, the buffer and the
    # generation.
    region = tmp_path / "seg.buf0.3"
    region.write_bytes(struct.pack("<16f", *([0.0] * 16)))

    with client.map_buffer(0) as mapped:
        assert (mapped.frames, mapped.channels) == (8, 2)
        assert len(mapped.samples) == 16
        mapped.samples[5] = 0.25
    assert struct.unpack_from("<f", region.read_bytes(), 20)[0] == 0.25, \
        "a write lands in the region, which is what the engine reads"


def test_a_retired_slot_stops_answering_even_with_the_file_there(tmp_path):
    """Freeing a buffer is the generation going **even**. The region is unlinked
    rather than deleted, so a peer that still holds a mapping keeps valid
    memory — what tells it the material is history is the directory."""
    path = _segment(tmp_path)
    client = ShmClient(str(path))
    at = client._buffers_at
    struct.pack_into("<Q", client.mm, at, 5)
    struct.pack_into("<II", client.mm, at + 8, 4, 1)
    struct.pack_into("<d", client.mm, at + 16, 48_000.0)
    (tmp_path / "seg.buf0.5").write_bytes(b"\0" * 16)
    assert client.map_buffer(0) is not None

    struct.pack_into("<Q", client.mm, at, 6)  # freed
    assert client.buffer_info(0) is None
    assert client.map_buffer(0) is None
