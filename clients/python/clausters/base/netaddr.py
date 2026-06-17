"""Network address of a server (port of ``sc3/base/netaddr.py``).

A plain target: host and port. It does not own a socket — the destination
interface (:mod:`clausters.base._oscinterface`) does the sending — so the same
``NetAddr`` works in RT or NRT. The clock carries the interface and the target
together; the convenience ``send_*`` methods route a one-off through a given
interface.
"""


class NetAddr:
    def __init__(self, host: str = "127.0.0.1", port: int = 57110):
        self.host = host
        self.port = port

    def addr(self) -> tuple[str, int]:
        return (self.host, self.port)

    def send_msg(self, interface, addr, *args):
        interface.send_msg(self.addr(), addr, *args)

    def send_bundle(self, interface, when, *messages):
        interface.send_bundle(self.addr(), when, *messages)

    def __repr__(self):
        return f"NetAddr({self.host!r}, {self.port})"
