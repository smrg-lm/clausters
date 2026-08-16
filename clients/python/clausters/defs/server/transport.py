"""The server's shared transport grid: the beat every client phases on.

The grid is one conductor's to define (`ServerTransport.set_transport`) and
everyone else's to join. Bound to a group it stops being advisory: the engine
freezes and thaws that subtree, so a stop is a real pause of the sound the
server is generating rather than a convention the clients observe.
"""

from ...base import _osclib
from ...errors import CommandError


class ServerTransport:
    """The transport half of `Server`; never instantiated on its own."""

    def transport(self, timeout: "float | None" = None):
        """The server's shared transport grid (``/transport_query``) as
        ``(origin_sample, tempo)``, or ``None`` if none is set. The grid lets
        several clients phase-align on the master clock; join it from a clock
        with `clausters.base.clock.TempoClock.join_transport`. RT only."""
        _, args = self.request("/transport_query", timeout=timeout, expect=("/transport_query.reply",))
        origin, tempo, defined = int(args[0]), float(args[1]), int(args[2])
        return (origin, tempo) if defined else None

    def set_transport(self, origin_sample: int, tempo: float, timeout: "float | None" = None):
        """Define the server's shared transport grid (``/transport_set``): beat 0 at
        ``origin_sample`` on the sample clock, advancing at ``tempo`` beats per
        second. One client (the conductor) sets it; the others
        `join_transport`. Last writer wins. Defining the grid resets the rolling
        state to stopped at position 0."""
        addr, args = self.request("/transport_set", _osclib.Int64(int(origin_sample)), float(tempo),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_set failed: {args}")
        return self

    def transport_state(self, timeout: "float | None" = None):
        """The full shared transport state as a dict ``{origin_sample, tempo,
        playing, position, group, transport_sample, position_sample, loop}``.

        **Always a dict**: the transport exists whether or not anyone has
        defined a beat grid, because rolling, stopping and saying where the
        piece is need no beats. ``origin_sample`` and ``tempo`` are ``None``
        while there is no grid, and ``position`` — the song-position *beat* —
        is 0 there, since there is nothing to measure it against; the sample
        spelling below is live either way. Read the grid alone with
        `transport`, which still answers ``None`` when none is set.

        ``playing`` is whether the transport is rolling. A
        `clausters.seq.timeline.Playhead` follows this with `follow_transport`.
        RT only.

        ``group`` is the governed group (`transport_group`) or ``None`` when
        nothing is bound.

        The last three are the piece's own axis. ``transport_sample`` is the
        transport **clock** — samples elapsed under the transport, held while it
        is stopped and monotonic, so a locate does not move it — while
        ``position_sample`` is where the transport **is in the piece**, which is
        what a playhead draws: it jumps to wherever a locate puts it and wraps
        inside ``loop``, a ``(start, end)`` pair of samples or ``None`` when
        looping is off. ``position_sample`` is read from the engine as of its
        last completed block, so a query issued in the same breath as a locate
        may still answer the previous place."""
        _, args = self.request("/transport_query", timeout=timeout, expect=("/transport_query.reply",))
        defined = bool(int(args[2]))
        group = int(args[5])
        loop_start, loop_end = int(args[8]), int(args[9])
        return {
            "origin_sample": int(args[0]) if defined else None,
            "tempo": float(args[1]) if defined else None,
            "playing": bool(int(args[3])),
            "position": float(args[4]),
            "group": None if group < 0 else group,
            "transport_sample": int(args[6]),
            "position_sample": int(args[7]),
            "loop": (loop_start, loop_end) if loop_end > loop_start else None,
        }

    def transport_group(self, group, timeout: "float | None" = None):
        """Bind the group the transport governs (``/transport_group``), or
        unbind with ``None``.

        This is what gives the transport its teeth. With no group bound it is a
        shared beat grid plus a rolling state that clients obey by choice. With
        one bound, the **engine** enforces it: `transport_stop` freezes that
        subtree and the server's transport clock, `transport_play` thaws them.
        Every node in the subtree keeps its internal state across the freeze, so
        a resume continues the sound rather than restarting it — which is the
        only thing a pause can mean for material the server generates itself.

        Freeing the group unbinds the transport, and unbinding thaws whatever it
        governed, so no frozen subtree is left with nobody to resume it."""
        arg = -1 if group is None else int(group)
        addr, args = self.request("/transport_group", arg,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_group failed: {args}")
        return self

    def sched_at_transport(self, target: int, *messages):
        """Schedule ``packet`` at an absolute sample on the **transport** axis
        (``/sched_atTransport``), the counterpart of ``/sched_at``'s device axis.

        Declaring the axis is not about disambiguation — classification is
        deterministic, and a client that bound the group knows which of its
        nodes are governed. It is about **verification**: the server compares
        the declaration against its own classification and fails when they
        disagree, instead of playing the bundle in the wrong place. Needs a
        group bound. ``messages`` are ``(addr, *args)`` tuples, as for
        `send_bundle`."""
        inner = _osclib.immediate_bundle(*[_osclib.message(*m) for m in messages])
        addr, args = self.request("/sched_atTransport", _osclib.Int64(int(target)), inner,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/sched_atTransport failed: {args}")
        return self

    def transport_play(self, position: "float | None" = None, timeout: "float | None" = None):
        """Start the shared transport rolling (``/transport_play``). With
        ``position`` playback starts from that song-position beat; without it,
        from where it last stopped or located. The server broadcasts the change
        to every `/server_notify` client, so all playheads following the transport roll
        together. Needs a grid defined (`set_transport`)."""
        extra = [float(position)] if position is not None else []
        addr, args = self.request("/transport_play", *extra,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_play failed: {args}")
        return self

    def transport_stop(self, timeout: "float | None" = None):
        """Stop the shared transport (``/transport_stop``); every following
        playhead halts. Broadcast to `/server_notify` clients."""
        addr, args = self.request("/transport_stop", timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_stop failed: {args}")
        return self

    def transport_locate_sample(self, sample: int, timeout: "float | None" = None):
        """Seek on the piece's own **sample** axis (``/transport_locateSample``).

        The sibling of `transport_locate`, which takes a beat: a sequencer
        locates by beat and an audio editor by frame, and converting either into
        the other on the client is how a rounding error gets into a seek. The
        beat position follows, so both readings of `transport_state` agree.
        Needs a grid defined; a negative sample clamps to 0."""
        addr, args = self.request("/transport_locateSample", _osclib.Int64(int(sample)),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_locateSample failed: {args}")
        return self

    def transport_loop(self, span: "tuple[int, int] | None" = None,
                       timeout: "float | None" = None):
        """Set (or clear, with ``None``) the span of the piece the transport
        loops inside (``/transport_loop``), in samples.

        The span is **half-open**: ``(0, n)`` over an ``n``-sample take plays
        every frame exactly once and joins its own start with no repeated frame.
        Turning a loop on does not move the piece — it keeps playing and wraps
        when it first reaches the end — and the wrap happens in the engine, so
        nothing has to be sent once a pass completes. An empty or inverted span
        raises. What a loop toggle remembers is the client's to keep: clearing
        forgets the span."""
        args_out = () if span is None else (_osclib.Int64(int(span[0])), _osclib.Int64(int(span[1])))
        addr, args = self.request("/transport_loop", *args_out,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_loop failed: {args}")
        return self

    def transport_locate(self, position: float, timeout: "float | None" = None):
        """Set the shared transport's song position (``/transport_locate``) —
        where play starts, or where it seeks to while playing. Every following
        playhead locates to it. Broadcast to `/server_notify` clients."""
        addr, args = self.request("/transport_locate", float(position),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_locate failed: {args}")
        return self
