"""`Transport`: play, pause, stop and locate, with the view's playhead in step.

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
a lane, milliseconds for an engraved page. `Transport` converts to the first
itself and takes `to_units` for the second, which is the whole of what a view
has to say about its units.

**A pass ends by itself.** `clausters.seq.Playhead` reports the end of its scan,
so `update` parks the cursor at the piece's end without the script timing it.
"""

from .. import _native

__all__ = ["Transport"]


class Transport:
    """Drive a `clausters.seq.Playhead` and a view's playhead line together.

    Args:
        host: the `clausters.gui.host.GuiHost` the widgets live on. May be
            ``None`` and set later (a view that is drawn before it is opened).
        ids: the widget ids showing the line — one id, a sequence of them, or a
            callable returning either, for a view that redraws (its lanes are
            new widgets, and the transport must find the current ones).
        source: ``source(at, **kw)`` starts a pass at beat ``at`` and returns the
            playing `clausters.seq.Playhead` (``None`` when there is nothing to
            play). It is called afresh on every play, so what sounds is always
            the material as it now stands.
        tempo: the clock's tempo in beats per second (the `TempoClock`
            convention — 2.0 is 120 bpm).
        sample_rate: the engine's sample rate. With ``tempo`` it fixes the
            beats→samples conversion the anchor is expressed in.
        to_units: ``to_units(beats)`` → the view's own units, for the static
            cursor. Defaults to beats→samples, which is what the timeline views
            use; an engraved page passes its beats→milliseconds.
        extent: ``extent()`` → the piece's length in beats, where `update` parks
            the cursor when a pass ends. Read on each use, so a piece that grew
            (a clip dragged past the end) ends where it now ends.
    """

    def __init__(self, host, ids, *, source, tempo: float, sample_rate: float,
                 to_units=None, extent=None, clock=None, governed: bool = False):
        self.host = host
        self.ids = ids
        self.source = source
        self.tempo = float(tempo)
        self.sample_rate = float(sample_rate)
        self.to_units = self.beats_to_samples if to_units is None else to_units
        self.extent = extent
        #: The clock the pass runs on, when there is one. A governed `pause`
        #: freezes it instead of stopping the playhead.
        self.clock = clock
        #: Whether a **server** transport governs the material (its
        #: `clausters.defs.server.Server.transport_group` is bound). Governed, a
        #: pause freezes the server's subtree and this clock rather than
        #: stopping the playhead, so `resume` continues the sound where it
        #: stopped instead of re-rendering it.
        self.governed = bool(governed)
        #: The server the anchor queries for its clock — the destination of the
        #: last `play`, or whatever `anchor` was given.
        self.server = None
        self._playhead = None
        self._at = 0.0       # the beat the cursor waits at while stopped
        self._ended = False  # the end of a pass was already parked (send it once)

    # ---- the unit bridge ----

    def beats_to_samples(self, beats: float) -> float:
        """Beats → samples of the engine clock, through the core's own time
        arithmetic (the seconds→samples rounding every client shares)."""
        secs = _native.beats_to_secs(self.tempo, 0.0, 0.0, float(beats))
        return float(_native.secs_to_samples(secs, self.sample_rate))

    def _targets(self) -> tuple:
        ids = self.ids() if callable(self.ids) else self.ids
        return (ids,) if isinstance(ids, int) else tuple(ids)

    # ---- the transport ----

    @property
    def playhead(self):
        """The `clausters.seq.Playhead` of the pass in flight, or ``None`` before
        the first `play`."""
        return self._playhead

    @property
    def playing(self) -> bool:
        """Whether a pass is rolling. It goes False on its own at the end of the
        piece, which is what `update` reads."""
        ph = self._playhead
        return ph is not None and ph.playing

    @property
    def position(self) -> float:
        """The transport's position in beats: where the playhead is while it
        plays, and where the next `play` starts when it does not."""
        ph = self._playhead
        return ph.position() if (ph is not None and ph.playing) else self._at

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
        keyword goes on to `source`. Returns the playhead."""
        if server is not None:
            self.server = server
        at = self._at if at is None else float(at)
        self._halt()
        self._at = at
        self._ended = False
        self._playhead = self.source(at, **kw)
        self.cursor(None)          # the clock's line takes over from the cursor
        self.anchor(at=at)
        return self._playhead

    def pause(self) -> float:
        """Halt where we are: the cursor stays on what the music stopped on, and
        `play` resumes from there. What is already sounding keeps sounding —
        stopping a playhead is not a panic button (the script owns its voices).
        Returns the position it stopped at.

        **Governed** (a server transport holds the material), the playhead is
        not stopped at all — it is starved of time. `/transport_stop` freezes
        the server's subtree and its queue, the clock freezes with them, and the
        scan simply stops making progress. That is what lets `resume` continue
        the sound rather than start it again."""
        ph = self._playhead
        if ph is not None and ph.playing:
            self._at = ph.position()
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
        return self._playhead

    def stop(self):
        """Halt and go back to the top."""
        self.pause()
        return self.locate(0.0)

    def locate(self, beat: float):
        """Seek: put the transport at ``beat``. Playing, it starts a fresh pass
        from there (so a seek also picks up any edit); stopped, it just moves the
        cursor the view draws. This is what a click on a ruler does."""
        beat = max(float(beat), 0.0)
        if self.playing:
            self.play(at=beat)
        else:
            self._at = beat
            if self._playhead is not None:
                self._playhead.locate(beat)   # the pass no longer ended *here*
            self._ended = False
            self.cursor(beat)
        return self

    def update(self) -> bool:
        """Park the cursor when the pass ends by itself. Call it once per pass of
        the script's loop; it returns whether the piece just ended.

        The playhead says when its scan ran out (`clausters.seq.Playhead.finished`),
        so the end needs no timing here: the cursor stops at the piece's `extent`
        rather than sweeping off the view, and stays there — the transport is
        *at the end*, so it is a `locate` (a rewind) that goes back to the top."""
        ph = self._playhead
        if self._ended or ph is None or not ph.finished:
            return False
        self._ended = True
        end = ph.position() if self.extent is None else self.extent()
        self._at = max(float(end), 0.0)
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
        ph = self._playhead
        if ph is not None and ph.playing:
            ph.stop()
