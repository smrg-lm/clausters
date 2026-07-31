"""Where OSC goes, and how a `Moment` becomes wire time.

A destination owns three things a `NetAddr` does not: the interface the bytes
leave through, the target they go to, and the policy that turns a logical
moment into a timetag. `clausters.defs.Server` is the destination we control --
it adds this server's latency, schedules by absolute sample when the clock is
anchored to the server's own, and accumulates a score offline. `OscDestination`
is every other one: standard OSC and nothing else.
"""

from typing import Protocol, runtime_checkable

from ._oscinterface import OscUdpInterface
from .moment import Moment
from .netaddr import NetAddr


@runtime_checkable
class Destination(Protocol):
    """Somewhere OSC goes.

    Note what is *not* here: ``play_event``. Rendering an `Event` is a
    double dispatch onto destinations that understand the server's node
    commands (`clausters.defs.Server`, a MIDI destination, a timeline); an
    external application does not know what ``/s_new`` is.
    """

    def send_msg(self, addr: str, *args) -> None:
        """Send one message, untimetagged."""
        ...

    def send_bundle(self, *messages, at: "Moment | None" = None,
                    delay_beats: float = 0.0) -> None:
        """Send a timetagged bundle of ``(addr, *args)`` messages."""
        ...


class OscDestination:
    """An OSC application we do not control.

    Standard OSC only: a message, or a bundle carrying an NTP timetag. No
    latency -- that is a property of *our* audio pipeline, and what another
    application needs is its own business, asked for as an explicit delay. No
    ``/sched`` (our command), no score (only our render reads one).

    The interface is created and closed by the destination unless one is
    passed, in which case it is borrowed and left alone.
    """

    def __init__(self, host: str = "127.0.0.1", port: int = 57120, interface=None):
        self.target = NetAddr(host, port)
        self.interface = interface if interface is not None else OscUdpInterface().start()
        self._owns_interface = interface is None

    def send_msg(self, addr: str, *args) -> None:
        """Send one message. **A message has no time** — it means "now"."""
        self.interface.send_msg(self.target.addr(), addr, *args)

    def send_bundle(self, *messages, at: "Moment | None" = None,
                    delay_beats: float = 0.0) -> None:
        """Emit a timetagged bundle of ``(addr, *args)`` messages at ``at``
        (default: the ambient moment) plus ``delay_beats``.

        Inside a routine that is the routine's exact logical beat, so a
        sequence sent to another application stays as tight as one sent to the
        server. Outside any routine it is wall-clock now plus the delay read as
        seconds.
        """
        when = (at if at is not None else Moment.current()).at(delay_beats)
        self.interface.send_bundle(self.target.addr(), when.instant(), *messages)

    def close(self) -> None:
        """Close the interface, if this destination opened it."""
        if self._owns_interface:
            self.interface.close()

    def __repr__(self):
        return f"OscDestination({self.target.host!r}, {self.target.port})"
