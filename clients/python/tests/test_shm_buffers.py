"""The **material** in a shared segment: what a local client maps rather than asks for.

A `clausters --shm <path>` server puts every pool buffer's samples in a region
beside the segment and indexes them in its directory, so a client on the same
machine reads and writes the take itself instead of fetching it. These check
the client's half of that: the directory it reads, the region it maps, and the
ring it talks through.

**The segments here are built by the shared core**, not by this test. That is
the whole change these tests were rewritten for: the layout used to be
transcribed into this client and asserted against a number transcribed into the
test, which is a mirror checked against itself — and it passed happily while
this binding declared 1024 control buses against a server that had had 16 384
for months. `clausters_core::shm` writes the header now, through
`_native.shm_init`, so a segment a test builds is a segment a server would.
"""

import ctypes
import mmap
import struct

import pytest

from clausters import _native
from clausters.ipc import ShmClient


def test_the_segment_size_is_the_one_the_server_builds():
    """Pinned on both sides: `tests/ipc.rs` asserts the same number in Rust.

    It is the default-count instance of the layout — 16384 control buses, the
    audio-bus region, 8 taps of 16384 samples, and 4096 directory rows — and it
    comes from the core rather than from arithmetic repeated here.
    """
    assert _native.shm_segment_size(16384, 8, 16384, 4096) == 722_624 + 4096 * 24


def _segment(tmp_path, control_buses=4, taps=1, tap_frames=64, buffers=4):
    """A segment as the server writes one, small enough to read by hand.

    Sized and initialised through the core, so what this test opens is a real
    segment rather than this file's idea of one.
    """
    size = _native.shm_segment_size(control_buses, taps, tap_frames, buffers)
    path = tmp_path / "seg"
    path.write_bytes(b"\0" * size)
    with open(path, "r+b") as handle:
        with mmap.mmap(handle.fileno(), 0) as mm:
            cell = (ctypes.c_char * size).from_buffer(mm)
            assert _native.shm_init(ctypes.addressof(cell), size,
                                    control_buses, taps, tap_frames)
            del cell
    return path


def test_a_segment_reports_the_shape_the_core_gave_it(tmp_path):
    client = ShmClient(str(_segment(tmp_path)))
    assert client.control_buses == 4
    assert (client.taps, client.tap_frames) == (1, 64)
    assert client.buffers == 4, "the directory is the segment's tail"
    assert client.shape.controls_offset > 0
    client.close()


def test_something_that_is_not_a_segment_is_refused(tmp_path):
    path = tmp_path / "junk"
    path.write_bytes(b"\0" * 4096)
    with pytest.raises(Exception):
        ShmClient(str(path))


def test_an_empty_slot_reads_as_no_buffer(tmp_path):
    client = ShmClient(str(_segment(tmp_path)))
    assert client.buffer_info(0) is None, "a zero generation is an empty slot"
    assert client.map_buffer(0) is None
    assert client.buffer_info(99) is None, "and a row that does not exist"
    client.close()


def test_a_published_buffer_is_read_and_mapped(tmp_path):
    """The whole point: the samples are the server's memory, not a copy of it."""
    path = _segment(tmp_path)
    client = ShmClient(str(path))

    # What the server writes when it installs a buffer: an **odd** generation,
    # then the shape. Written here through the directory's own offsets, which
    # the core reported.
    at = client.shape.buffers_offset
    struct.pack_into("<Q", client.mm, at, 3)
    struct.pack_into("<II", client.mm, at + 8, 8, 2)
    struct.pack_into("<d", client.mm, at + 16, 48_000.0)
    assert client.buffer_info(0) == (3, 8, 2, 48_000.0)

    # ...and the region beside it, named from the segment, the buffer and the
    # generation — a name the core builds, so three processes agree on it.
    region = tmp_path / ("seg" + _native.shm_region_suffix(0, 3))
    region.write_bytes(struct.pack("<16f", *([0.0] * 16)))

    with client.map_buffer(0) as mapped:
        assert (mapped.frames, mapped.channels) == (8, 2)
        assert len(mapped.samples) == 16
        mapped.samples[5] = 0.25
    assert struct.unpack_from("<f", region.read_bytes(), 20)[0] == 0.25, \
        "a write lands in the region, which is what the engine reads"
    client.close()


def test_a_retired_slot_stops_answering_even_with_the_file_there(tmp_path):
    """Freeing a buffer is the generation going **even**. The region is unlinked
    rather than deleted, so a peer that still holds a mapping keeps valid
    memory — what tells it the material is history is the directory."""
    path = _segment(tmp_path)
    client = ShmClient(str(path))
    at = client.shape.buffers_offset
    struct.pack_into("<Q", client.mm, at, 5)
    struct.pack_into("<II", client.mm, at + 8, 4, 1)
    struct.pack_into("<d", client.mm, at + 16, 48_000.0)
    (tmp_path / ("seg" + _native.shm_region_suffix(0, 5))).write_bytes(b"\0" * 16)
    assert client.map_buffer(0) is not None

    struct.pack_into("<Q", client.mm, at, 6)  # freed
    assert client.buffer_info(0) is None
    assert client.map_buffer(0) is None
    client.close()


def test_the_ring_carries_a_packet_and_reads_it_back(tmp_path):
    """The command plane, framed by the core: a client pushes into c2s, and
    what a server would drain is exactly what was pushed."""
    path = _segment(tmp_path)
    client = ShmClient(str(path))
    assert client.poll() is None, "nothing to read yet"
    assert client.send(b"/status\0", peer=3)
    # The server's end of the same pair, which is what this client wrote into.
    assert _native.shm_pop(client._addr, len(client.mm), 0) == (3, b"/status\0")
    client.close()
