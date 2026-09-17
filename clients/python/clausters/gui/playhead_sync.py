"""`PlayheadSync`: play, pause, stop and locate, with the views' playhead in step.

Every time view the host draws — a lane, a piano-roll, an engraved page — shows
the same line, and every script that plays into one needs the same four buttons.
This is that logic, once, independent of which widget it drives.

**The line is the host's, not the client's.** ``playhead_at`` is a single
anchor: the sample-clock value the view's time 0 maps to. The host reads the
engine's clock every frame and draws the line from there, so a pass costs *one*
message, not one per frame. A transport that is not playing is the other half of
that number — ``playhead_at`` goes negative and the static ``playhead`` holds
the cursor where the music was left, which is what makes pause look like pause.

**Two axes meet here.** The anchor lives on the engine's sample clock (samples,
always); the static cursor lives on the *view's* own axis — timeline samples for
a lane, milliseconds for an engraved page. `PlayheadSync` converts to the first
itself and takes `to_units` for the second, which is the whole of what a view
has to say about its units.

**It holds no tempo.** Beats cross to samples through the map of **what plays**:
the pass `source` returned (a `clausters.seq.Timeline` holds its own map), else
the ``structure`` it was given. A view represents a structure's data and keeps
none of it, so a tempo edited on the structure is the one the line follows.

**A pass ends by itself.** A `clausters.seq.Timeline` reports that it finished,
so `update` parks the cursor at the piece's end without the script timing it.

**Or the server owns all of it** (``head_clock="piece"``). Then none of the paragraphs
above apply: the transport is the audio server's, the position is the engine's
`positionSample` -- held while stopped, moved by a locate, wrapped inside a loop
in the engine -- and this class is four commands and a read. Play, pause, seek
and loop stop being a line kept in step and become `/transport_play`,
`/transport_stop`, `/transport_locateSample` and `/transport_loop`; the anchor
is 0, because the counter the host draws already *is* the piece's time. That is
the shape a multitrack wants, where many readers follow one time
(``TransportPos``), and it is why an editor sends a locate and reads a position
back instead of computing one.
"""

from .. import _native

__all__ = ["PlayheadSync"]

#: How often a rolling transport asks itself whether the pass has ended, in
#: seconds. It is not the line's frame rate — the host sweeps that from the
#: engine's clock without being told — only how sharply the cursor parks at the
#: end of the piece.
TICK = 0.05


class PlayheadSync:
    """Keep the views' playhead line in step with what plays, and relay the
    transport verbs to it.

    Args:
        host: the `clausters.gui.host.GuiHost` the widgets live on. May be
            ``None`` and set later (a view that is drawn before it is opened).
        ids: the widget ids showing the line — one id, a sequence of them, or a
            callable returning either, for a view that redraws (its lanes are
            new widgets, and the transport must find the current ones).
        source: ``source(at, **kw)`` starts a pass at beat ``at`` and returns
            what plays -- a `clausters.seq.Timeline` played from there (``None``
            when there is nothing to play). It is called afresh on every play,
            so what sounds is always the structure as it now stands.
        structure: what is played, asked for its tempo map (``map``) when no
            pass is in flight -- a `clausters.seq.Timeline`, or a callable
            returning the object that has one. The map is never kept here.
        sample_rate: the engine's sample rate. With the map it fixes the
            beats→samples conversion the anchor is expressed in.
        to_units: ``to_units(beats)`` → the view's own units, for the static
            cursor. Defaults to beats→samples, which is what the timeline views
            use; an engraved page passes its beats→milliseconds.
        extent: ``extent()`` → the piece's length in beats, where `update` parks
            the cursor when a pass ends. Read on each use, so a piece that grew
            (a clip dragged past the end) ends where it now ends.
        head_clock: which counter the view's line is drawn from —  ``"device"``
            (the default: the engine's sample clock, which never stops, so this
            class owns the piece's time and anchors the line to it) or
            ``"piece"`` (the **server's transport position**, so the server owns
            it and this class sends commands). Setting ``"piece"`` also tells
            the host (`clausters.gui.host.GuiHost.head_clock`), because the two
            are one decision and letting them disagree draws a line nobody put
            there. A view whose axis is not samples keeps ``"device"``: the
            piece's position is measured in frames.
    """

    def __init__(self, host, ids, *, source=None, structure=None,
                 sample_rate: float, to_units=None, extent=None, clock=None,
                 governed: bool = False, head_clock: str = "device"):
        self.host = host
        self.ids = ids
        self.source = source
        #: What is played, asked for its map when no pass is in flight.
        self.structure = structure
        self.sample_rate = float(sample_rate)
        self.to_units = self.beats_to_samples if to_units is None else to_units
        self.extent = extent
        #: The clock the pass runs on, when there is one. A governed `pause`
        #: freezes it instead of stopping the playhead.
        self.clock = clock
        #: Whether a **server** transport governs the samples (its
        #: `clausters.defs.server.Server.transport_group` is bound). Governed, a
        #: pause freezes the server's subtree and this clock rather than
        #: stopping the playhead, so `resume` continues the sound where it
        #: stopped instead of re-rendering it.
        self.governed = bool(governed)
        #: Which counter the line is drawn from: ``"device"`` or ``"piece"``,
        #: the same two words `clausters.gui.host.GuiHost.head_clock` takes.
        #: In ``"piece"`` the position, the rolling state, the seek and the loop
        #: are all the audio server's, and this class holds none of them.
        self.head_clock = str(head_clock)
        #: The server the anchor queries for its clock — the destination of the
        #: last `play`, or whatever `anchor` was given.
        self.server = None
        self._playhead = None
        self._at = 0.0       # the beat the cursor waits at while stopped
        self._ended = False  # the end of a pass was already parked (send it once)
        #: The **tail**: `(clock beat, timeline beat)` at the moment the scan
        #: drained. A scan runs out when it renders its *last item*, not when
        #: the piece is over — the last clip is still sounding, and the line
        #: must go on crossing it. `None` outside that stretch.
        self._tail = None
        self._ticking = False  # a self-driven `update` is scheduled
        #: The last answer `refresh` got from the server's transport, on the
        #: piece. Empty until one is asked for.
        self._piece = {}
        if self.head_clock == "piece" and host is not None and hasattr(host, "head_clock"):
            host.head_clock("piece")

    # ---- the unit bridge ----

    def tempo_map(self):
        """The map beats cross to samples through, asked for on each use: the
        pass in flight's (a `clausters.seq.Timeline` holds its own), else the
        ``structure``'s. The line sweeps by engine samples from an origin this
        places, so the origin has to come from the function the sound plays by.
        """
        tempo_map = getattr(self._playhead, "map", None)
        if tempo_map is not None:
            return tempo_map
        structure = self.structure() if callable(self.structure) else self.structure
        tempo_map = getattr(structure, "map", None)
        if tempo_map is None:
            raise ValueError("PlayheadSync: nothing to read a tempo map from; "
                             "give it the structure it plays")
        return tempo_map

    def beats_to_samples(self, beats: float) -> float:
        """Beats → samples of the engine clock, through the piece's time map
        (and the core's seconds→samples rounding every client shares).

        Where the line's origin comes from, so it must be the map and not a
        ratio: the host sweeps the playhead by engine samples, and a beat placed
        by a frozen tempo would be crossed at a time the clock never plays it
        at.
        """
        secs = self.tempo_map().secs_at(float(beats))
        return float(_native.secs_to_samples(secs, self.sample_rate))

    def _targets(self) -> tuple:
        ids = self.ids() if callable(self.ids) else self.ids
        return (ids,) if isinstance(ids, int) else tuple(ids)

    # ---- the transport ----

    @property
    def playhead(self):
        """What the pass in flight plays -- the `clausters.seq.Timeline`
        `source` returned -- or ``None`` before the first `play`."""
        return self._playhead

    @property
    def playing(self) -> bool:
        """Whether the piece is sounding: a pass is rolling, **or** its scan has
        drained and the last item is still ringing (the tail). It goes False on
        its own at the end of the piece — where the last item ends, not where it
        started — which is what `update` decides.

        The tail counts as playing because everything a caller does with this
        answer is true of it: a pause holds where the music is, a seek starts a
        fresh pass from there, and a button reads "pause" rather than "play".

        On the **piece** it is the engine's last answer (`refresh`) and none of
        the above: the transport is rolling or it is not, and nothing here has an
        opinion."""
        if self.head_clock == "piece":
            return bool(self._piece.get("playing"))
        ph = self._playhead
        return (ph is not None and ph.playing) or self._tail is not None

    @property
    def position(self) -> float:
        """The transport's position in beats: where the playhead is while it
        plays, where it got to while the last item is still ringing, and where
        the next `play` starts when neither.

        On the **piece** it is what the engine last said (`refresh`), not
        something kept here, which is the whole point: a wrap at a loop's end and
        a seek some other client sent are both where it says, and neither passed
        through this object. Asking is a round trip and this is not, so a caller
        that wants it current refreshes first -- the *line* needs neither, since
        the host draws it straight from the segment every frame."""
        if self.head_clock == "piece":
            return self.samples_to_beats(self._piece.get("position_sample", 0))
        ph = self._playhead
        if ph is not None and ph.playing:
            return ph.position()
        tail = self._tail_position()
        return self._at if tail is None else tail

    def _tail_position(self):
        """Where the line is between the scan draining and the piece ending: the
        last item's beat plus what the clock has advanced since, never past the
        end. ``None`` when there is no tail to be in.

        The clock is the **pass's own** (a timeline's hidden one), and it
        has to be *rolling*: an offline render computes the whole piece in an
        instant and its beat is the queue's, not the wall's, so there is no tail
        to sweep and the cursor parks straight away — exactly as it did before
        this existed."""
        if self._tail is None:
            return None
        since, beat = self._tail
        clock = self._pass_clock()
        if clock is None or not getattr(clock, "rolling", False):
            return beat
        end = beat if self.extent is None else float(self.extent())
        return min(beat + (clock.beats() - since), max(end, beat))

    def refresh(self):
        """Ask the server where the piece is, and remember it; returns ``self``.

        **The read is separate from the answer** because asking is a round trip
        and `position` is not: a counter refreshes on its own tick, a button
        reads what is already known, and the *line* refreshes neither — the host
        draws it straight from the segment, every frame, with nothing sent. On a
        device-clock transport this does nothing, since the position is here.

        (In the web client this is a promise, for the reason every request there
        is one: a page waits for an answer instead of blocking on it. The call
        is the same call.)
        """
        server = self.server
        if self.head_clock == "piece" and server is not None \
                and hasattr(server, "transport_state"):
            self._piece = server.transport_state()
        return self

    def reported(self, *, playing: "bool | None" = None,
                 position_sample: "int | None" = None):
        """**What the engine was just told**, on the piece: whether it rolls and
        where it stands, remembered as `refresh` would have answered, and every
        target's line drawn from the piece's position again. Returns ``self``.

        For a caller that sent the transport's commands itself -- a playback
        whose verbs are the shared crate's -- so the answers this object gives
        before the next `refresh` are the ones the commands made true."""
        if playing is not None:
            self._piece["playing"] = bool(playing)
        if position_sample is not None:
            self._piece["position_sample"] = int(position_sample)
        self._piece_anchor()
        return self

    def samples_to_beats(self, samples: float) -> float:
        """Samples of the piece → beats, through the same map `beats_to_samples`
        goes the other way — so what the engine reports and what the ruler draws
        are one function read in two directions."""
        secs = float(samples) / self.sample_rate if self.sample_rate > 0 else 0.0
        return self.tempo_map().beats_at(secs)

    def _piece_anchor(self):
        """Draw every target's line straight from the piece's position: the
        anchor is 0, because the counter the host reads already is that time.

        Re-applied rather than set once, because a view that redraws has new
        widgets and they come up with no line at all."""
        if self.host is None:
            return
        for wid in self._targets():
            self.host.set(wid, playhead_at=0.0, playhead=-1.0)

    def _pass_clock(self):
        """The clock the pass in flight runs on: its own (a timeline's hidden
        clock), else the one this was given."""
        ph = self._playhead
        own = getattr(ph, "clock", None) or getattr(getattr(ph, "_player", None), "clock", None)
        return own or self.clock

    @property
    def at(self) -> float:
        """The beat a bare `play` starts from — where a `pause`, a `locate` or the
        end of a pass left the transport. It is *not* `position`: a play while
        already playing restarts from here, not from where the music got to."""
        return self._at

    def play(self, server=None, *, at: float | None = None, **kw):
        """Play (or resume) from beat ``at`` — the transport's position by
        default — and anchor the line to the engine clock. ``server`` is where
        the anchor's clock query goes (remembered for later passes); any other
        keyword goes on to `source`. Returns what the pass plays.

        On the **piece** it is `/transport_play`, and a bare one: the engine
        keeps where it stopped, so resuming is the same verb as starting and
        nothing is re-rendered. Given an ``at`` it seeks there first."""
        if server is not None:
            self.server = server
        if self.head_clock == "piece":
            if at is not None:
                self.locate(at)
            self._piece_anchor()
            # A `source` is still called, and it is the **events** half: what
            # follows the transport by itself (a reader on ``TransportPos``)
            # needs no pass, and what fires voices does. So the two halves of a
            # piece meet here -- the engine's readers, and a client pass the
            # transport's verbs cue.
            self._halt()
            if self.source is not None:
                self._playhead = self.source(self.position if at is None else at, **kw)
            if self.server is not None and hasattr(self.server, "transport_play"):
                self.server.transport_play()
            self._piece["playing"] = True
            return self._playhead
        at = self._at if at is None else float(at)
        self._halt()
        self._at = at
        self._ended = False
        self._playhead = self.source(at, **kw)
        self.cursor(None)          # the clock's line takes over from the cursor
        self.anchor(at=at)
        self._watch()
        return self._playhead

    def pause(self) -> float:
        """Halt where we are: the cursor stays on what the music stopped on, and
        `play` resumes from there. What is already sounding keeps sounding —
        stopping a playhead is not a panic button (the script owns its voices).
        Returns the position it stopped at.

        **Governed** (a server transport holds the samples), the playhead is
        not stopped at all — it is starved of time. `/transport_stop` freezes
        the server's subtree and its queue, the clock freezes with them, and the
        scan simply stops making progress. That is what lets `resume` continue
        the sound rather than start it again."""
        if self.head_clock == "piece":
            # Nothing to park and nothing to compute: the engine holds the
            # position where it froze, and the line holds with it.
            if self.server is not None and hasattr(self.server, "transport_stop"):
                self.server.transport_stop()
            self._piece["playing"] = False
            # Governed, the pass is starved of time rather than stopped, so
            # resuming continues it; ungoverned there is nothing to starve.
            if self.governed:
                if self.clock is not None:
                    self.clock.freeze()
            else:
                self._halt()
            return self.position
        # Where the music stopped — including inside the tail, where the scan
        # has drained but the last clip is still sounding.
        self._at = self.position
        if self.governed:
            server = self.server
            if server is not None and hasattr(server, "transport_stop"):
                server.transport_stop()
            if self.clock is not None:
                self.clock.freeze()
        else:
            self._halt()
        self.cursor(self._at)
        return self._at

    def resume(self):
        """Continue from where `pause` left off, **without re-rendering**.

        The difference from `play` is MIDI's `continue` versus `start`: play
        reads the composition as it now stands and starts it again from `at`,
        resume picks the frozen sound back up. Governed, the server still holds
        every node's internal state and every scheduled bundle, so what comes
        back is the same sound carried on — a stochastic texture continues
        mid-gesture instead of restarting. Ungoverned there is nothing frozen to
        continue, so this falls back to `play`."""
        if not self.governed:
            return self.play()
        server = self.server
        if server is not None and hasattr(server, "transport_play"):
            server.transport_play()
        if self.clock is not None:
            self.clock.thaw()
        self._ended = False
        self.anchor(at=self.position)
        self._watch()
        return self._playhead

    def stop(self):
        """Halt and go back to the top."""
        self.pause()
        return self.locate(0.0)

    def locate(self, beat: float):
        """Seek: put the transport at ``beat``. Playing, it starts a fresh pass
        from there (so a seek also picks up any edit); stopped, it just moves the
        cursor the view draws. This is what a click on a ruler does.

        On the **piece** it is one `/transport_locateSample`, playing or not:
        the seek happens in the engine, so nothing is re-cued and what is
        already sounding carries on from there rather than being cut and
        started again."""
        beat = max(float(beat), 0.0)
        if self.head_clock == "piece":
            if self.server is not None and hasattr(self.server, "transport_locate_sample"):
                self.server.transport_locate_sample(int(self.beats_to_samples(beat)))
            self._piece["position_sample"] = int(self.beats_to_samples(beat))
            self._piece_anchor()
            # The readers seek in the engine and need nothing; a pass of voices
            # has to be cued again, which is the one re-cue the piece keeps --
            # on a locate, and not on every edit.
            if self.playing and self.source is not None:
                self._halt()
                self._playhead = self.source(beat)
            return self
        if self.playing:
            self.play(at=beat)
        else:
            self._tail = None
            self._at = beat
            if self._playhead is not None:
                self._playhead.locate(beat)   # the pass no longer ended *here*
            self._ended = False
            self.cursor(beat)
        return self

    def loop(self, start: float | None = None, end: float | None = None):
        """The span of the piece the position wraps inside, in beats — or, with
        no arguments (or ``None``), looping off.

        **The piece's only**, because it is the only one the engine can wrap:
        the wrap happens on its exact sample, so a pass repeats with no seam and
        no client in the loop. A device-clock transport folds the *drawn* line
        instead (``playhead_loop_*``), which is a different thing and stays the
        view's.
        """
        if self.head_clock != "piece":
            raise ValueError("a loop is the piece's: build the transport with "
                             'head_clock="piece"')
        server = self.server
        if server is None or not hasattr(server, "transport_loop"):
            return self
        if start is None or end is None:
            server.transport_loop(None)
        else:
            server.transport_loop((int(self.beats_to_samples(max(start, 0.0))),
                                   int(self.beats_to_samples(max(end, 0.0)))))
        return self

    def _watch(self):
        """Have the end of the pass noticed, without a script asking.

        `update` is the question "has it ended yet", and somebody has to ask it.
        That used to be the script's own loop — which is how every example came
        to have one — and it is now the host's
        `clausters.base.appclock.AppClock`, the same thread the window's
        gestures arrive on. A transport with no host (a view built but never
        opened) simply keeps `update` as the manual call it always was.
        """
        if self._ticking:
            return
        clock = getattr(self.host, "clock", None)
        if clock is None:
            return
        self._ticking = True
        clock.sched(TICK, self._tick)

    def _tick(self):
        """One look, then another in `TICK` seconds while there is still
        something to notice.

        Returning a number is how the loop's timer reschedules, so this is a
        periodic task with no loop of its own; returning ``None`` ends it.

        "Still something to notice" is **not** `playing`: a scan that has just
        run out is not playing and is exactly the moment `update` exists for, so
        stopping there would leave the cursor sweeping off the end forever. It
        is the piece sounding, or a drained scan that has not been parked yet —
        and a `pause`, which keeps its playhead without ending it, stops the
        asking until the next `play`."""
        ph = self._playhead
        if ph is None or self._ended or not (
                self.playing or bool(getattr(ph, "finished", False))):
            self._ticking = False
            return None
        self.update()
        return TICK

    def update(self) -> bool:
        """Park the cursor when the pass ends by itself; it returns whether the
        piece just ended.

        **Nothing has to call this.** A `play` schedules it on the host's
        application clock for as long as the piece is sounding, so a script
        neither loops nor ticks. It stays public because a transport built
        before its host exists has no clock to schedule on, and because asking
        the question once more is always legal.

        The pass says when it ran out (`clausters.seq.Timeline.finished`),
        so the end needs no timing here: the cursor stops at the piece's `extent`
        rather than sweeping off the view, and stays there — the transport is
        *at the end*, so it is a `locate` (a rewind) that goes back to the top."""
        ph = self._playhead
        if self._ended or ph is None or not ph.finished:
            return False
        end = float(ph.position() if self.extent is None else self.extent())
        clock = self._pass_clock()
        if end > ph.position() and getattr(clock, "rolling", False):
            if self._tail is None:
                # From the moment the last item was *rendered* — which is a
                # loop pass or two before anyone noticed — not from now.
                since = getattr(ph, "scanned_at", None)
                if since is None and getattr(ph, "_player", None) is not None:
                    # A timeline's clock beat *is* its beat, and it holds the
                    # beat its last item fell on.
                    since = ph.position()
                self._tail = (clock.beats() if since is None else since,
                              ph.position())
            if self._tail_position() < end:
                return False       # the last clip is still sounding
        self._ended = True
        self._tail = None
        self._at = max(end, 0.0)
        self.cursor(self._at)
        return True

    # ---- the line: anchored to the clock, or a static cursor ----

    def anchor(self, server=None, *, at: float = 0.0) -> bool:
        """Anchor the view's playhead to the engine clock, so the line starts at
        beat ``at`` and sweeps on with the audio. Returns whether it could.

        The anchor is a **query**: it asks the server for its clock, and a server
        that does not answer leaves the view without a line — so the failure is
        reported, not swallowed (a playhead that silently never appears is the
        worst of both). A destination with no engine clock — an NRT score — has
        nothing to anchor to and answers False.
        """
        from ..errors import ReplyTimeout

        if server is not None:
            self.server = server
        server = self.server
        if self.host is None or server is None or not hasattr(server, "request"):
            return False
        if getattr(getattr(server, "interface", None), "time_mode", "unix") == "score":
            return False  # NRT: there is no engine clock to anchor to
        try:
            _addr, args = server.request("/clock_query", expect=("/clock_query.reply",))
        except ReplyTimeout:
            return False  # a live server that did not answer: no line, and it shows
        if not args:
            return False
        # Items sound `latency` ahead of the time they were played at, so the
        # clock value beat 0 maps to is *now* plus that latency, less what has
        # already been played.
        now = float(args[0]) + getattr(server, "latency", 0.0) * self.sample_rate
        origin = now - self.beats_to_samples(at)
        for wid in self._targets():
            self.host.set(wid, playhead_at=origin)
        return True

    def unanchor(self):
        """Take the sweeping line off the view (the static cursor stays). The
        host's anchored playhead *tracks the engine clock*, so a line left
        anchored keeps sweeping after the music stopped."""
        self.cursor(self._at)
        return self

    def cursor(self, beat):
        """Draw (or clear) the static cursor — the located position of a
        transport that is not playing. ``None`` clears it, which is what the
        clock anchor does when a pass takes the line over."""
        if self.host is None:
            return self
        pos = -1.0 if beat is None else self.to_units(beat)
        for wid in self._targets():
            self.host.set(wid, playhead_at=-1.0, playhead=pos)
        return self

    def _halt(self):
        """Stop the pass in flight, if any, without touching the cursor."""
        self._tail = None
        ph = self._playhead
        if ph is not None and ph.playing:
            (getattr(ph, "pause", None) or ph.stop)()
