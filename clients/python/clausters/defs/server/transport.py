"""The server's shared transport grid: the beat every client phases on.

The grid is one conductor's to define (`ServerTransport.set_transport`) and
everyone else's to join. Bound to a group it stops being advisory: the engine
freezes and thaws that subtree, so a stop is a real pause of the sound the
server is generating rather than a convention the clients observe.

A server has several transports (its ``--transports``), each independent --
its own grid, rolling state, position, loop, end mark and governed group. The
methods here address transport 0 on a `Server`; `ServerTransport.transport_at`
answers the same server addressed through another one, so anything written
against a server's transport -- a `clausters.seq.Timeline`, a playback --
plays on whichever it is handed.
"""

from ...base import _osclib
from ...errors import CommandError
from ..node import _target_id


def _is_transport(transport: int):
    """A reply matcher: whether a ``/transport_query.reply`` is about
    ``transport`` -- the pushes of every transport share its address."""
    def match(args) -> bool:
        return len(args) > 12 and int(args[12]) == transport
    return match


class ServerTransport:
    """The transport half of `Server`; never instantiated on its own."""

    #: The transport this handle addresses: 0 on a `Server`, the one it was
    #: made for on what `transport_at` answers.
    transport_id: int = 0

    def transport_at(self, transport: int) -> "Transport":
        """**Transport ``transport`` of this server, as an object**: a
        `Transport`, the same one every time it is asked for. Its verbs --
        `Transport.play`, `Transport.pause`, `Transport.locate`, ... -- are
        that transport's, and it goes wherever a transport is taken:
        ``timeline.transport = server.transport_at(n)``. The methods on the
        server itself address transport 0, and that is the one transport a
        script names by number on its own: every other is taken by what plays
        -- a sequence, an audio editor, a GUI host's monitor -- so a number
        picked by hand may be somebody's. `transport_new` takes a free one;
        this addresses one already known, an editor's ``transport.id`` say.
        An id past the server's ``--transports`` fails when a command is
        sent, not here."""
        server = getattr(self, "_server", self)
        held = server.__dict__.setdefault("_transports", {})
        found = held.get(int(transport))
        if found is None:
            found = held[int(transport)] = Transport(server, int(transport))
        return found

    def transport_new(self) -> "Transport":
        """**A transport nobody holds, taken from this server's**, as a
        `Transport`: for a timeline of its own, a group to govern apart from
        everything else that plays. It is the caller's until `Transport.free`
        gives it back.

        Raises:
            RuntimeError: when every transport is taken -- a server has a
                fixed number of them (``--transports``), and a GUI host
                sharing the server takes half.
        """
        from ... import _native

        server = getattr(self, "_server", self)
        taken = server.ids.alloc(_native.IdSpaces.TRANSPORTS)
        if taken is None:
            raise RuntimeError(
                "out of transports: every one this client may allocate is in "
                "use; free one, or boot the server with more (--transports)")
        transport = server.transport_at(taken)
        transport._taken = True
        return transport

    def _transport_query(self, timeout):
        """``/transport_query`` for this handle's transport, answered by the
        reply about it and not by another transport's push."""
        addr, args = self.request("/transport_query", self.transport_id, timeout=timeout,
                                  expect=("/transport_query.reply", "/fail"),
                                  match=_is_transport(self.transport_id))
        if addr == "/fail":
            raise CommandError(f"/transport_query failed: {args}")
        return args

    def transport(self, timeout: "float | None" = None):
        """The server's shared transport grid (``/transport_query``) as
        ``(origin_sample, tempo)``, or ``None`` if none is set. The grid lets
        several clients phase-align on the master clock; join it from a clock
        with `clausters.base.clock.TempoClock.join_transport`. RT only."""
        args = self._transport_query(timeout)
        origin, tempo, defined = int(args[0]), float(args[1]), int(args[2])
        return (origin, tempo) if defined else None

    def set_transport(self, origin_sample: int, tempo: float, timeout: "float | None" = None):
        """Define the server's shared transport grid (``/transport_set``): beat 0 at
        ``origin_sample`` on the sample clock, advancing at ``tempo`` beats per
        second. One client (the conductor) sets it; the others
        `join_transport`. Last writer wins. Defining the grid resets the rolling
        state to stopped at position 0."""
        addr, args = self.request("/transport_set", self.transport_id,
                                  _osclib.Int64(int(origin_sample)), float(tempo),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_set failed: {args}")
        return self

    def transport_state(self, timeout: "float | None" = None):
        """The full shared transport state as a dict ``{origin_sample, tempo,
        playing, position, group, transport_sample, position_sample, loop,
        end, transport, follow, fade}``, ``transport`` being this handle's
        `transport_id`, ``follow`` the group that follows it
        (`transport_follow`) or ``None``, and ``fade`` the length of a stop's
        and a play's ramp in samples (`transport_fade`), 0 for none.

        **Always a dict**: the transport exists whether or not anyone has
        defined a beat grid, because rolling, stopping and saying where the
        the transport is need no beats. ``origin_sample`` and ``tempo`` are ``None``
        while there is no grid, and ``position`` -- the song-position *beat* --
        is 0 there, since there is nothing to measure it against; the sample
        spelling below is live either way. Read the grid alone with
        `transport`, which still answers ``None`` when none is set.

        ``playing`` is whether the transport is rolling. A
        A `clausters.seq.Timeline` on this transport
        (`clausters.seq.Timeline.transport`) follows it. RT only.

        ``group`` is the governed group (`transport_group`) or ``None`` when
        nothing is bound.

        The last three are the transport's own axis. ``transport_sample`` is the
        transport **clock** -- samples elapsed under the transport, held while it
        is stopped and monotonic, so a locate does not move it -- while
        ``position_sample`` is where the transport **stands**, which is
        what a playhead draws: it jumps to wherever a locate puts it and wraps
        inside ``loop``, a ``(start, end)`` pair of samples or ``None`` when
        looping is off. ``position_sample`` is read from the engine as of its
        last completed block -- except right after a locate, which the server
        answers with the place it located to until a block has applied it, so a
        reply in the same breath as a locate (its own broadcast above all) never
        reports the place the transport is leaving.

        ``end`` is the end mark (`transport_end`) as an ``(end, back)`` pair --
        ``back`` ``None`` when the position rests on the mark -- or ``None``
        when none is set."""
        args = self._transport_query(timeout)
        defined = bool(int(args[2]))
        group = int(args[5])
        loop_start, loop_end = int(args[8]), int(args[9])
        end = int(args[10]) if len(args) > 10 else -1
        back = int(args[11]) if len(args) > 11 else -1
        return {
            "origin_sample": int(args[0]) if defined else None,
            "tempo": float(args[1]) if defined else None,
            "playing": bool(int(args[3])),
            "position": float(args[4]),
            "group": None if group < 0 else group,
            "transport_sample": int(args[6]),
            "position_sample": int(args[7]),
            "loop": (loop_start, loop_end) if loop_end > loop_start else None,
            "end": None if end < 0 else (end, None if back < 0 else back),
            "transport": self.transport_id,
            "follow": None if len(args) < 14 or int(args[13]) < 0 else int(args[13]),
            "fade": int(args[14]) if len(args) > 14 else 0,
        }

    def transport_group(self, group, timeout: "float | None" = None):
        """Bind the group the transport governs (``/transport_group``), or
        unbind with ``None``. ``group`` is a `clausters.defs.node.Group` or its
        raw id, like every other place a node is named.

        This is what gives the transport its teeth. With no group bound it is a
        shared beat grid plus a rolling state that clients obey by choice. With
        one bound, the **engine** enforces it: `transport_stop` freezes that
        subtree and the server's transport clock, `transport_play` thaws them.
        Every node in the subtree keeps its internal state across the freeze, so
        a resume continues the sound rather than restarting it -- which is the
        only thing a pause can mean for sound the server generates itself.

        Freeing the group unbinds the transport, and unbinding thaws whatever it
        governed, so no frozen subtree is left with nobody to resume it."""
        arg = -1 if group is None else _target_id(group)
        addr, args = self.request("/transport_group", self.transport_id, arg,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_group failed: {args}")
        return self

    def transport_follow(self, group, timeout: "float | None" = None):
        """Have ``group`` **follow** the transport (``/transport_follow``), or end
        that with ``None``. ``group`` is a `clausters.defs.node.Group` or its
        raw id.

        Its nodes read the transport -- its position, whether it rolls -- as a
        governed group's do, and nothing freezes them. It is for the part of an
        application that must go on running while its transport is stopped, an
        output with its meter and its declick, and still has to know that
        transport. The governed group may sit inside it. A transport has one
        following group, as it has one governed group, and a group is bound to
        one transport either way."""
        arg = -1 if group is None else _target_id(group)
        addr, args = self.request("/transport_follow", self.transport_id, arg,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_follow failed: {args}")
        return self

    def lane_new(self, lane: int, target, timeout: "float | None" = None):
        """Make **event lane** ``lane`` on this transport (``/lane_new``): notes
        and messages the transport plays by its position, as a reader plays a
        take -- a locate moves them, a loop plays them again on every pass, a
        stop releases them, with nothing sent per pass. ``lane`` is an id the
        caller picks, like a buffer's; its notes are made at the tail of
        ``target`` (a `clausters.defs.node.Group` or its raw id), which should
        be a group this transport does **not** govern: a stop releases the
        notes, and a voice frozen there would sound again, mid-release, on the
        next play. An existing lane of that id is freed first."""
        addr, args = self.request("/lane_new", self.transport_id, int(lane),
                                  _target_id(target),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/lane_new failed: {args}")
        return self

    def lane_set(self, lane: int, data: dict, timeout: "float | None" = None):
        """Replace event lane ``lane``'s data whole (``/lane_set``):
        ``{"notes": [[start, end, voice, {control: value}, "gate" | "free"],
        ...], "messages": [[position, address, *args], ...], "midi":
        [[position, *bytes], ...], "ump": [[position, *words], ...]}``,
        every position a sample of the
        transport's position and every list optional. A note's ``voice`` is a
        def's name, or ``{"graph": id, "slot": name}`` for one more of a slot
        of a running graph instance, the controls its ports -- how a note
        carries the curves that shape it. A note is released at ``end`` by
        ``gate 0`` or by a free; a message is a command the server
        takes in a timed bundle, run as written; a MIDI message plays as though
        it had reached the server's MIDI input there, through its channel's
        ``/midi_bind`` binding, and a ``ump`` entry -- one MIDI 2.0 packet's
        words -- the same way at its own resolution, its per-note messages
        reaching the note on their channel and key. What sounds
        keeps its release, and the new data is heard from where the position
        is. The notes editor's playback writes it from a
        `clausters.seq.EventSequence`."""
        import json

        addr, args = self.request("/lane_set", int(lane), json.dumps(data),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/lane_set failed: {args}")
        return self

    def lane_free(self, lane: int, timeout: "float | None" = None):
        """Free event lane ``lane`` (``/lane_free``): what it queued is dropped
        and the notes it is sounding are released."""
        addr, args = self.request("/lane_free", int(lane),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/lane_free failed: {args}")
        return self

    def sched_at_transport(self, target: int, *messages):
        """Schedule ``packet`` at an absolute sample on the **transport** axis
        (``/sched_atTransport``), the counterpart of ``/sched_at``'s device axis.

        Declaring the axis is not about disambiguation -- classification is
        deterministic, and a client that bound the group knows which of its
        nodes are governed. It is about **verification**: the server compares
        the declaration against its own classification and fails when they
        disagree, instead of playing the bundle in the wrong place. Needs a
        group bound. ``messages`` are ``(addr, *args)`` tuples, as for
        `send_bundle`."""
        inner = _osclib.immediate_bundle(*[_osclib.message(*m) for m in messages])
        addr, args = self.request("/sched_atTransport", self.transport_id,
                                  _osclib.Int64(int(target)), inner,
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
        addr, args = self.request("/transport_play", self.transport_id, *extra,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_play failed: {args}")
        return self

    def transport_stop(self, timeout: "float | None" = None):
        """Stop the shared transport (``/transport_stop``); every following
        playhead halts. Broadcast to `/server_notify` clients."""
        addr, args = self.request("/transport_stop", self.transport_id,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_stop failed: {args}")
        return self

    def transport_locate_sample(self, sample: int, timeout: "float | None" = None):
        """Seek on the transport's own **sample** axis (``/transport_locateSample``).

        The sibling of `transport_locate`, which takes a beat: a sequencer
        locates by beat and an audio editor by frame, and converting either into
        the other on the client is how a rounding error gets into a seek. The
        beat position follows, so both readings of `transport_state` agree.
        Needs a grid defined; a negative sample clamps to 0."""
        addr, args = self.request("/transport_locateSample", self.transport_id,
                                  _osclib.Int64(int(sample)),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_locateSample failed: {args}")
        return self

    def transport_loop(self, span: "tuple[int, int] | None" = None,
                       timeout: "float | None" = None):
        """Set (or clear, with ``None``) the span of the axis the transport
        loops inside (``/transport_loop``), in samples.

        The span is **half-open**: ``(0, n)`` over an ``n``-sample take plays
        every frame exactly once and joins its own start with no repeated frame.
        Turning a loop on does not move the transport -- it keeps playing and wraps
        when it first reaches the end -- and the wrap happens in the engine, so
        nothing has to be sent once a pass completes. An empty or inverted span
        raises. What a loop toggle remembers is the client's to keep: clearing
        forgets the span."""
        args_out = () if span is None else (_osclib.Int64(int(span[0])), _osclib.Int64(int(span[1])))
        addr, args = self.request("/transport_loop", self.transport_id, *args_out,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_loop failed: {args}")
        return self

    def transport_end(self, end: "int | None" = None, back: "int | None" = None,
                      timeout: "float | None" = None):
        """Set (or clear, with ``None``) the transport's **end mark**
        (``/transport_end``), in samples: where a rolling transport stops, and
        ``back``, where it is located once it has -- ``None`` leaves it on the
        mark.

        The stop is the engine's, on the mark's exact sample, and it is a stop
        like `transport_stop`: the governed group and the transport clock
        freeze, and every `/server_notify` client is told. The mark stays set,
        so the next play from ``back`` ends at the same place, and the transport
        never rolls past it: a play from at or past the mark stops at once.
        **A loop wins** -- while one is set the position wraps and never
        reaches the mark."""
        args_out = () if end is None else (
            (_osclib.Int64(int(end)),) if back is None
            else (_osclib.Int64(int(end)), _osclib.Int64(int(back))))
        addr, args = self.request("/transport_end", self.transport_id, *args_out,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_end failed: {args}")
        return self

    def transport_fade(self, samples: int, timeout: "float | None" = None):
        """How long a stop and a play **ramp** (``/transport_fade``), in
        samples; 0, the default, is no ramp.

        With a ramp a stop is a **stopping phase**: the governed group goes on
        running and the position goes on advancing while the ramp falls to
        zero, and then they freeze -- the position rests where the readers
        stopped reading. A play thaws and ramps up. What reads the ramp is
        `clausters.defs.transport_fade`, in a group that follows the transport
        (`transport_follow`): an output multiplies what the readers wrote by
        it, and neither edge clicks. The end mark starts its ramp that long
        before the mark, so the pass still ends on it; a loop's wrap and a
        locate are not ramped."""
        addr, args = self.request("/transport_fade", self.transport_id,
                                  _osclib.Int64(int(samples)),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_fade failed: {args}")
        return self

    def transport_locate(self, position: float, timeout: "float | None" = None):
        """Set the shared transport's song position (``/transport_locate``) --
        where play starts, or where it seeks to while playing. Every following
        playhead locates to it. Broadcast to `/server_notify` clients."""
        addr, args = self.request("/transport_locate", self.transport_id, float(position),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_locate failed: {args}")
        return self


class _Addressed(ServerTransport):
    """A `Server` addressed through one of its transports: its transport
    methods name ``transport_id``, a ``sched_clear("transport")`` clears that
    transport's queue alone, and everything else is the server's own. What a
    `Transport` sends its commands through, and what a timeline on it plays
    against."""

    def __init__(self, server, transport: int):
        self._server = server
        self.transport_id = int(transport)

    @property
    def server(self):
        """The server this view addresses."""
        return self._server

    def request(self, addr, *args, **options):
        return self._server.request(addr, *args, **options)

    def sched_clear(self, axis: "str | None" = None):
        """The server's `sched_clear`, with ``"transport"`` meaning this
        transport's queue."""
        return self._server.sched_clear(axis, transport=self.transport_id)

    def __getattr__(self, name):
        return getattr(self._server, name)

    def __repr__(self):
        return f"<transport {self.transport_id} of {self._server!r}>"


class Transport:
    """**One of a server's transports, as an object**: what
    `ServerTransport.transport_at` answers, and what ``play(sequence)``
    answers for the transport the sequence took -- one of its own, so two
    sequences play together, each driven by the object its ``play`` answered.

    It is played the way a routine or a timeline is: `play`, `pause`, `stop`,
    `locate`, `loop` and `unloop`, `playing`, and `wait`, which a script calls
    or not -- a live session drives the transport with the same verbs and never
    waits. Its positions are those of **what is loaded on it**: the beats of
    the sequence a ``play(sequence)`` put there, and with nothing loaded, the
    transport's own seconds.

    A sequence keeps its transport until it is freed: `free` releases what
    sounds, frees its lane and gives the transport back to the server's, which
    has a fixed number of them (``--transports``). They go with the server's
    handle when it is closed.

    The transport's other commands are here by their own names too: `group`
    and `follow` bind the groups it governs and leads, `fade` sets how a stop
    and a play ramp, `locate_sample` seeks on its sample axis, and `state` is
    what the engine says of it. A server's own transport methods
    (``transport_play``, ...) address transport 0; this object is how any
    other is addressed.
    """

    def __init__(self, server, transport: int):
        self._server = server
        self._id = int(transport)
        #: What its commands are sent through.
        self._view = _Addressed(server, self._id)
        #: What is loaded on it and plays through it, when something is: the
        #: playback of a sequence, which speaks its beats.
        self._driver = None
        #: Whether `ServerTransport.transport_new` took it for a script, whose
        #: `free` then gives it back.
        self._taken = False
        self._rate = None
        #: The span and the loop switch with nothing loaded, in seconds.
        self._span = None
        self._looping = False

    @property
    def server(self):
        """The server whose transport this is."""
        return self._server

    @property
    def id(self) -> int:
        """Which of the server's transports it is."""
        return self._id

    def _seconds(self, secs: float) -> int:
        if self._rate is None:
            self._rate = float(self._server.query_info().nominal_sample_rate)
        return int(round(float(secs) * self._rate))

    # ---- played as a routine is ----

    def state(self) -> dict:
        """The transport as the engine has it (``/transport_query``): see
        `ServerTransport.transport_state`."""
        return self._view.transport_state()

    @property
    def playing(self) -> bool:
        """Whether the transport is rolling, as the engine answers -- a pass
        that ended on its own stopped with nobody here saying so."""
        if self._driver is not None:
            return self._driver.playing()
        return bool(self.state()["playing"])

    def play(self, at: "float | None" = None) -> "Transport":
        """Roll -- from ``at`` when given, else from where it was paused, or
        located, or from the start. Returns ``self``."""
        if self._driver is not None:
            self._driver.play(at)
            return self
        if at is not None:
            self.locate(at)
        self._view.transport_play()
        return self

    def pause(self) -> "Transport":
        """Stop where it stands: a `play` carries on from there."""
        if self._driver is not None:
            self._driver.pause()
            return self
        self._view.transport_stop()
        return self

    def stop(self) -> "Transport":
        """Stop and go back to where the pass started. What is sounding is
        released, and rings out."""
        if self._driver is not None:
            self._driver.stop()
            return self
        self._view.transport_stop()
        self._view.transport_locate_sample(0)
        return self

    def locate(self, at: float) -> "Transport":
        """Put the position at ``at``: a rolling transport goes on from there,
        a stopped one starts there next."""
        if self._driver is not None:
            self._driver.locate(float(at))
            return self
        self._view.transport_locate_sample(self._seconds(at))
        return self

    @property
    def span(self):
        """**The time range** ``(start, end)`` a pass plays and a loop
        repeats, or ``None``. With a sequence loaded it is the range a sweep
        leaves on its roll -- set it here and the roll draws it, sweep it there
        and it reads here -- and `play` plays it, from its start to its end,
        as the space bar does. Kept while stopped."""
        if self._driver is not None:
            return self._driver.span
        return self._span

    @span.setter
    def span(self, span) -> None:
        if self._driver is not None:
            self._driver.set_span(span)
            return
        self._span = None if span is None else (float(span[0]), float(span[1]))
        if self._looping:
            self._loop_raw()

    @property
    def looping(self) -> bool:
        """Whether the loop switch is on."""
        if self._driver is not None:
            return self._driver.looping
        return self._looping

    def loop(self, start: "float | None" = None, end: "float | None" = None) -> "Transport":
        """**Loop**: with ``start`` and ``end``, set the `span` to them first;
        then turn the loop on over the span -- or, with none, over every note
        of the sequence loaded. A rolling transport follows at once, a stopped
        one on its next `play`. The `L` key over a roll is the same switch."""
        if start is not None and end is not None:
            self.span = (start, end)
        if self._driver is not None:
            self._driver.set_looping(True)
            return self
        self._looping = True
        self._loop_raw()
        return self

    def unloop(self) -> "Transport":
        """Turn the loop off; the `span` stays."""
        if self._driver is not None:
            self._driver.set_looping(False)
            return self
        self._looping = False
        self._view.transport_loop(None)
        return self

    def _loop_raw(self) -> None:
        if self._span is None:
            return
        self._view.transport_loop((self._seconds(self._span[0]), self._seconds(self._span[1])))

    @property
    def end(self):
        """**Where a pass ends**: ``None``, the transport rolling on until it
        is stopped; a position, an end marker; or, with a sequence loaded,
        ``"contents"``, where its last note ends -- what ``play(sequence)``
        sets. A pass that reaches its end stops there, and `wait` returns."""
        if self._driver is not None:
            return self._driver.end
        end = self.state().get("end")
        if end is None:
            return None
        self._seconds(0.0)
        return end[0] / self._rate

    @end.setter
    def end(self, end) -> None:
        if self._driver is not None:
            self._driver.set_end(end)
            return
        if end == "contents":
            raise ValueError("a transport with nothing loaded has no contents")
        self._view.transport_end(None if end is None else self._seconds(end))

    def free(self) -> "Transport":
        """**Free what is loaded on it, and give it back**: the sequence a
        ``play(sequence)`` put there stops, its notes released, its lane is
        freed, and the transport goes back to the server's for something else
        to take -- this object is then a transport with nothing loaded. One
        `ServerTransport.transport_new` answered goes back the same way. A
        transport addressed by number has nothing to free."""
        from ... import _native

        if self._driver is not None:
            self._driver.free()
        elif self._taken:
            self._taken = False
            self._server.ids.release(_native.IdSpaces.TRANSPORTS, self._id)
        return self

    def wait(self, timeout: "float | None" = None) -> bool:
        """Block until the transport stops -- a pass that ends where its
        contents do stops on its own -- or ``timeout`` seconds pass. Answers
        whether it stopped. A script that plays and exits calls it; a live
        session does not."""
        import time

        deadline = None if timeout is None else time.monotonic() + float(timeout)
        while self.playing:
            if deadline is not None and time.monotonic() >= deadline:
                return False
            time.sleep(0.05)
        return True

    # ---- the transport's other commands ----

    def group(self, group) -> "Transport":
        """Bind the group this transport governs: see
        `ServerTransport.transport_group`."""
        self._view.transport_group(group)
        return self

    def follow(self, group) -> "Transport":
        """Have ``group`` follow this transport: see
        `ServerTransport.transport_follow`."""
        self._view.transport_follow(group)
        return self

    def fade(self, samples: int) -> "Transport":
        """How long a stop and a play ramp, in samples: see
        `ServerTransport.transport_fade`."""
        self._view.transport_fade(samples)
        return self

    def locate_sample(self, sample: int) -> "Transport":
        """Seek on the transport's own sample axis: see
        `ServerTransport.transport_locate_sample`."""
        self._view.transport_locate_sample(sample)
        return self

    def lane_new(self, lane: int, target) -> "Transport":
        """Make event lane ``lane`` on this transport: see
        `ServerTransport.lane_new`. Its data and its end are the server's
        (`ServerTransport.lane_set`, `ServerTransport.lane_free`)."""
        self._view.lane_new(lane, target)
        return self

    def sched_clear(self) -> "Transport":
        """Drop what is queued on this transport's clock (``/sched_clear
        "transport"``)."""
        self._view.sched_clear("transport")
        return self

    def __repr__(self):
        return f"<Transport {self._id} of {self._server!r}>"
