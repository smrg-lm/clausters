"""C8 client: OscTcpInterface framing and reply reassembly.

Pure-unit, no live server: a fake socket records what the interface sends and
feeds back canned bytes, so the length-prefix framing and the across-segments
reassembly are checked deterministically. The live round-trip is exercised by
the Rust integration test (`tests/osc.rs::tcp_*`) and by the examples."""

from clausters.base import OscTcpInterface
from clausters.base import _osclib as osc


class FakeSocket:
    """Minimal socket stand-in: stores sent bytes; serves recv() chunks from a
    queue (an empty chunk = a closed/timed-out read)."""

    def __init__(self, recv_chunks=()):
        self.sent = bytearray()
        self._recv = list(recv_chunks)
        self.timeout = None

    def sendall(self, data):
        # What a real socket does when a timeout is set and the send buffer is
        # momentarily full: it raises rather than waiting. The fake raises
        # unconditionally, since what is being asserted is that no timeout is
        # set at all by the time a send happens.
        if self.timeout is not None:
            raise TimeoutError("timed out")
        self.sent += data

    def recv(self, _n):
        return self._recv.pop(0) if self._recv else b""

    def settimeout(self, t):
        self.timeout = t

    def setsockopt(self, *a):
        pass

    def close(self):
        pass


def _iface(chunks=()) -> OscTcpInterface:
    iface = OscTcpInterface()
    iface._sock = FakeSocket(chunks)      # bypass start()/the network
    return iface


def _unframe(blob: bytes):
    assert len(blob) >= 4
    length = int.from_bytes(blob[:4], "big")
    assert len(blob) == 4 + length        # exactly one frame, fully sent
    return blob[4:]


def test_send_msg_is_length_prefixed():
    iface = _iface()
    iface.send_msg(("127.0.0.1", 57110), "/synth_new", "default", 1000, 1, 0)
    addr, args = osc.decode(_unframe(bytes(iface._sock.sent)))
    assert addr == "/synth_new"
    assert args[:4] == ["default", 1000, 1, 0]


def test_send_bundle_is_framed():
    iface = _iface()
    iface.send_bundle(("127.0.0.1", 57110), 1.0, ("/server_status",))
    payload = _unframe(bytes(iface._sock.sent))
    assert payload[:8] == b"#bundle\x00"     # an OSC bundle


def test_recv_reassembles_a_frame_split_across_segments():
    reply = osc.message("/server_status.reply", 1, 0, 0, 1, 1)
    frame = len(reply).to_bytes(4, "big") + reply
    # Deliver the frame in three awkward pieces: part of the prefix, the rest of
    # the prefix plus part of the payload, then the tail.
    chunks = [frame[:2], frame[2:6], frame[6:]]
    iface = _iface(chunks)
    got = iface.recv(timeout=1.0)
    assert got == reply
    addr, _ = osc.decode(got)
    assert addr == "/server_status.reply"


def test_recv_returns_none_when_no_data():
    iface = _iface([])                    # recv() yields b"" → closed/timeout
    assert iface.recv(timeout=0.05) is None


def test_two_frames_in_one_chunk_are_returned_one_at_a_time():
    a = osc.message("/done", "/def_send")
    b = osc.message("/server_status.reply", 1)
    blob = (len(a).to_bytes(4, "big") + a) + (len(b).to_bytes(4, "big") + b)
    iface = _iface([blob])                # both frames arrive together
    assert osc.decode(iface.recv(1.0))[0] == "/done"
    assert osc.decode(iface.recv(1.0))[0] == "/server_status.reply"


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001
                print(f"FAIL {name}: {e}")
                traceback.print_exc()


def test_a_read_leaves_the_socket_blocking_for_the_next_send():
    """A timeout in Python belongs to the **socket**, not to the call that set
    it, so one left behind by a read governs the next ``sendall`` as well.

    Found by use 2026-08-21: `gui_analyzer` died on a window resize. The host
    asked the server for everything it had to redraw, the script's send buffer
    backed up for an instant, and the control sweep's ``synth.set`` raised
    ``TimeoutError`` instead of waiting a moment -- a send this client never
    gave a deadline to, holding one inherited from a read. `recv` walks its
    budget down to a remainder, so after a request that spends it the socket is
    left with microseconds on it, which is why the window died rather than
    stuttering.
    """
    iface = _iface([b""])                 # one read that comes back empty
    assert iface.recv(0.01) is None
    assert iface._sock.timeout is None, "the read left its timeout on the socket"
    # The send that was dying. It goes out because nothing is set any more.
    iface.send_msg(("127.0.0.1", 57110), "/node_set", 1000, "freq", 440.0)
    addr, args = osc.decode(_unframe(bytes(iface._sock.sent)))
    assert addr == "/node_set"
    assert args[:3] == [1000, "freq", 440.0]
