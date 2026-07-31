"""Network address of a server (port of ``sc3/base/netaddr.py``).

A plain target: host and port, and nothing else. It does not own a socket and
it does not send -- the **destination** does both
(`clausters.base.destination`), because sending needs an interface and a policy
for turning logical time into wire time, and neither of those is an address.
The same ``NetAddr`` therefore works in RT or NRT, behind any destination.

It *is* a tuple, so it goes straight to the socket calls that take one, and the
names are there for the code that reads a host or a port on its own.

It carries **no defaults**: which host and port to reach is the caller's, and a
default here would be one particular destination's address (the server's)
frozen into the type that stands for all of them.
"""

from typing import NamedTuple


class NetAddr(NamedTuple):
    host: str
    port: int
