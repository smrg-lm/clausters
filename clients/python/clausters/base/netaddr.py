"""Network address of a server (port of ``sc3/base/netaddr.py``).

A plain target: host and port, and nothing else. It does not own a socket and
it does not send -- the **destination** does both
(`clausters.base.destination`), because sending needs an interface and a policy
for turning logical time into wire time, and neither of those is an address.
The same ``NetAddr`` therefore works in RT or NRT, behind any destination.
"""


class NetAddr:
    def __init__(self, host: str = "127.0.0.1", port: int = 57110):
        self.host = host
        self.port = port

    def addr(self) -> tuple[str, int]:
        return (self.host, self.port)

    def __eq__(self, other):
        if not isinstance(other, NetAddr):
            return NotImplemented
        return (self.host, self.port) == (other.host, other.port)

    def __hash__(self):
        return hash((self.host, self.port))

    def __repr__(self):
        return f"NetAddr({self.host!r}, {self.port})"
