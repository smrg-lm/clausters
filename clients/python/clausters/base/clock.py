"""TempoClock (port of ``sc3/base/clock.py``, native-backed).

The seam between the native core and the host language. The clock owns the
scheduling queue and the beat/second arithmetic — the latter delegated to
``clausters-core`` through `clausters._native`, so timing matches the
server's sample clock. The queue holds **routines** (and one-shot callables);
resuming a routine (the ``yield`` driver) stays in Python.

One clock, two drives:

- `run` / `start` — real time: a background thread sleeps between
  events using a **monotonic** pacing clock; the logical beat still advances
  only by the routines' ``yield``s, so inter-event timing is exact and the OSC
  timetags (stamped from a separate wall clock) carry that exactness.
- `render` — non-real time: drain the queue in beat order with no
  sleeping, advancing a logical clock; used to build a score.

The clock does **not** talk to the server: it only schedules and exposes the
current time (`beats`, `beats2secs`, `start_time`). Sending
events belongs to `clausters.defs.server.Server`, which owns the
destination/communication interface and reads the time from the clock of the
routine being resumed (the clock sets ``routine.clock`` and
``main.current_routine`` around each wake). Swapping that interface (RT/NRT/MIDI) is
the seam — and it lives on the Server, not here.
"""

import atexit
import json as _json
import sys
import threading
import time
import traceback

from .. import _native
from .stream import Stream, StopStream, resume
from .timebase import MonotonicTimebase, SampleClockTimebase


class TempoClock:
    """A scheduler that keeps musical time in beats and resumes routines on it.

    A clock has a `tempo` (beats per second) and a queue of scheduled items --
    routines and one-shot callables. Two drives share that queue:

    - real time (`start` / `run`): a background thread sleeps between items,
      pacing against the `timebase`, and fires them live.
    - non-real time (`render`): the queue is drained in beat order with no
      sleeping, advancing a logical clock as fast as possible -- used to build
      a score offline.

    The defining property is that the **logical beat advances only by the
    routines' ``yield``s**, never by wall-clock drift: a routine that yields
    ``0.25`` is resumed exactly a quarter-beat later, whichever drive is running
    and whatever the OS scheduler does. That is what makes inter-event timing
    exact -- and, with a `SampleClockTimebase`, sample-accurate.

    The clock does not talk to the server. It only schedules and reports time
    (`beats`, `beats2secs`, `start_time`); a `Server` reads the clock of the
    routine it is resuming and emits from there. Choosing where events go (real
    time, offline, MIDI) is the Server's job, not the clock's.

    Args:
        tempo: beats per second.
        timebase: the pacing source -- the default monotonic clock, or a
            `SampleClockTimebase` to anchor pacing and scheduling to the
            server's own sample clock.
        tempo_map: a `clausters._native.TempoMap` to **read** instead of
            building one. Every clock builds its own, so this is only for the
            case where two clocks are reading one piece.
        name: a label. Says *what* this clock is (``"lead"``, ``"canon 3"``),
            never which one it is -- the same rule a document node's name
            follows. It is what a saved clock is recognised by when an
            arrangement is written against it.
    """

    def __init__(self, tempo: float = 1.0, timebase=None, tempo_map=None, name=None):
        #: What this clock is called, or ``None``. A label, not an identity.
        self.name = name
        #: The piece's beat->second map (`clausters._native.TempoMap`), and the
        #: clock's whole relation to time. It starts as one constant-tempo
        #: segment, which computes exactly the affine expression this clock
        #: always used; `set_tempo` records a breakpoint on it instead of
        #: overwriting the one anchor there used to be, so what a tempo change
        #: moved stays knowable afterwards.
        #:
        #: Every clock builds its own, so nothing has to be passed for the
        #: ordinary case; `tempo_map=` hands it one to **read** instead, which
        #: is how two clocks come to be reading one piece.
        self._map = tempo_map if tempo_map is not None else _native.TempoMap(float(tempo))
        # The last segment as an affine triple, refreshed on every edit: the
        # anchor `tempo = x` re-hangs the map from, so assigning a tempo keeps
        # the beat the clock is on where it already was. `_tempo` is that
        # segment's tempo -- the *destination* while a ramp is running; the
        # tempo actually sounding is the `tempo` property, read from the map.
        self._base_beats = 0.0
        self._base_secs = 0.0
        self._tempo = float(tempo)

        #: pacing source — *only* used to decide how long to sleep between
        #: events. The default is the OS monotonic clock; pass a
        #: `SampleClockTimebase` to anchor to the
        #: server's sample clock. The Server reads this to choose how to stamp
        #: events (NTP timetag vs ``/sched_at`` absolute sample).
        self.timebase = timebase if timebase is not None else MonotonicTimebase()
        self._now = self.timebase

        #: the beat-ordered queue lives in the native core (`clausters-core`'s
        #: `Scheduler`); only beats and flat ids cross, and `_items` maps each
        #: id back to its routine (holding the strong reference while queued).
        self._queue = _native.Scheduler()
        self._items = {}              # id -> [item, pending_count]
        self._cond = threading.Condition()
        self._mode = "stopped"        # 'rt' | 'nrt' | 'stopped'
        self._logical_beat = 0.0      # current beat while driving (yield-exact)
        self._mono_start = None       # pacing origin (monotonic)
        self._unix_start = None       # wall-clock origin for OSC timetags
        self._running = False
        self._thread = None
        #: whether anything ever drove this clock (`start`, `run` or `render`).
        #: A queued routine on a clock nobody drives never runs and says
        #: nothing, which looks exactly like silence — see `_warn_if_undriven`.
        self._driven = False
        self._exit_hook = False
        self._sample_clock = None     # the master-clock tracker, set by lock_to()
        self._transport = None        # joined shared beat grid, set by join_transport()
        #: The timebase reading at which `freeze` stopped the beat, or ``None``
        #: while the clock runs normally. See `freeze`.
        self._frozen_at = None
        #: the session this clock belongs to, so a play running on it resolves
        #: that session's server/rng (``current_routine.clock.session``).
        #:
        #: **A clock built while a session is ambient adopts it**, and the
        #: session keeps it and closes it. That is not a convenience: a routine
        #: runs on its clock's thread, and `Session.activate` is thread-local,
        #: so this back-reference is the *only* thing an ambient play can follow
        #: from inside a routine. A clock built with no session ambient has
        #: ``None`` here and resolves against the default session.
        self.session = None
        # Deferred, like the one in `_wake`: `main` reaches back here through
        # `get_default_clock`.
        from .main import main as _main

        if _main.current_session is not None:
            _main.current_session.adopt(self)

    # ---- beat/second math (native) ----

    @property
    def tempo(self) -> float:
        """Beats per second **at the beat the clock is on** — the tempo that is
        sounding, read from the map (`map.tempo_at(beats())`).

        Under a constant tempo, and after a ramp has finished, this is the last
        change's tempo. *Inside* a ramp it is the tempo reached so far, not the
        one being ramped to: the destination is `map.last()`, and a piece whose
        map has changes still ahead of the playhead has not reached them.

        Assigning it changes the slope without pinning the instant, which is
        what setting the grid does; `set_tempo` is the musical gesture (it keeps
        the current beat on the second it already fell on).
        """
        return self._map.tempo_at(self.beats())

    @tempo.setter
    def tempo(self, tempo: float):
        self._map = _native.TempoMap.anchored(
            float(tempo), self._base_beats, self._base_secs
        )
        self._sync_map()

    @property
    def map(self):
        """The clock's `clausters._native.TempoMap` — the piece's beat<->second
        function, readable without a clock running and shared with whatever
        draws the piece, so a line and the sound come from one map.

        Assigning it hands the clock a piece's own tempo, and the map is
        **adopted, not copied**: a second clock assigned the same map is reading
        the same piece, and a gesture written on either is written on both. Pass
        ``m.copy()`` to fork instead.

        Do it before `start` — replacing the map under a running clock moves
        every beat that has not fired yet, which is a seek and not a tempo
        change.

        **What a shared map costs, and it is one thing.** The driver recomputes
        every wait from the map on each pass, so an edit made anywhere is
        *read* correctly with nothing to invalidate. What a clock cannot see is
        an edit made through another holder while it sleeps: it wakes on the
        wait it had already computed, and only then reads the new map. Its own
        gestures (`set_tempo`, `tempo`) wake it at once; for an edit written
        from elsewhere, call `resync` on the clocks reading it — or compare
        `map.version`, which is what it is there for.
        """
        return self._map

    @map.setter
    def map(self, tempo_map):
        self._map = tempo_map
        self._sync_map(wake=True)

    def dump(self) -> str:
        """The clock as JSON: its name and its tempo map.

        **What of a clock belongs to the piece, and it is only these two.** Its
        position is transport, its queue is what happens to be scheduled, and
        its `timebase` is a choice of the *run* -- whether it paces against the
        OS clock or the server's sample counter says nothing about the music.
        What the piece owns is the tempo, and the name a lane refers to it by.

        This is what an arrangement written at a tempo saves: not "the" tempo,
        which would make polytempo unwritable, but a named clock per tempo,
        with lanes naming which one they run on.
        """
        return _json.dumps({"name": self.name, "map": _json.loads(self._map.dump())})

    @classmethod
    def load(cls, json: str, timebase=None) -> "TempoClock":
        """A clock rebuilt from what `dump` wrote: the same name, the same
        tempo map, and a `timebase` that is this run's rather than the saved
        one's (there is no saved one -- see `dump`). Raises `ValueError` on
        anything this client could not have written.
        """
        try:
            data = _json.loads(json)
            name, points = data["name"], data["map"]
        except (TypeError, KeyError, ValueError) as exc:
            raise ValueError(f"not a saved clock: {exc}") from exc
        clock = cls(timebase=timebase, tempo_map=_native.TempoMap.load(_json.dumps(points)))
        clock.name = name
        return clock

    def resync(self):
        """Re-read the map and wake the driver — after an edit written through
        another holder of a **shared** map.

        A clock's own gestures do this for you. This is the call for the other
        direction: a piece's map edited by an editor, or by a second clock, and
        this one still asleep on a wait computed before the edit.
        """
        self._sync_map(wake=True)
        return self

    def _sync_map(self, wake: bool = False):
        """Re-reads the map's last segment into the affine cache, and (for an
        edit) wakes the driver, which may be asleep on a wait the edit just
        moved."""
        self._base_beats, self._base_secs, self._tempo = self._map.last()
        if wake:
            with self._cond:
                self._cond.notify_all()

    def beats2secs(self, beats: float) -> float:
        """Convert a beat position to seconds through the piece's time map
        (computed in the native core, so it matches the server's own
        arithmetic). Under one tempo this is the affine conversion it has always
        been; across a tempo change it is the integral, so a beat before the
        change still reports the second it actually fell on."""
        return self._map.secs_at(beats)

    def secs2beats(self, secs: float) -> float:
        """Convert seconds to a beat position through the piece's time map (the
        inverse of `beats2secs`; native core, server-matching)."""
        return self._map.beats_at(secs)

    def beats(self) -> float:
        """The clock's current beat: the monotonic-paced elapsed beat while
        running in RT (what scheduling relative to "now" reads), else the
        yield-driven logical beat — while rendering, before the first `start`,
        and after a `stop`, which holds the beat it reached."""
        if self._mode == "nrt" or not self._running or self._mono_start is None:
            return self._logical_beat
        if self._frozen_at is not None:
            return self.secs2beats(self._frozen_at - self._mono_start)
        return self.secs2beats(self._now() - self._mono_start)

    @property
    def queued(self) -> int:
        """How many items are in the schedule queue right now.

        What `clear` empties and `unsched` shrinks -- read for the same reason
        the web client's tests read it: to say a routine actually left the
        queue, rather than that it stopped producing."""
        return len(self._queue)

    @property
    def rolling(self) -> bool:
        """Whether the beat advances **by itself**: the real-time driver is
        running (`start`, not yet `stop`ped).

        False before the first `start` and during an offline `render`, whose
        beat is the queue's position and not the wall's — the distinction a
        caller needs before treating `beats` as a thing that moves while it
        waits (a transport sweeping a cursor over the last item's tail).
        Freezing does not change it: a frozen clock is rolling and held."""
        return self._running and self._mode == "rt"

    # ---- the freeze gate (a server transport's pause, reaching the clock) ----

    @property
    def frozen(self) -> bool:
        """Whether the beat is held where `freeze` left it."""
        return self._frozen_at is not None

    def freeze(self):
        """Hold the logical beat where it is, without stopping the clock.

        This is how a server transport's pause reaches a client. The sample
        timebase only decides how long to sleep between events and how to stamp
        one, so a client whose server froze would otherwise keep advancing beats
        and scheduling events ahead — running away from a piece that is not
        moving. Freezing stops the beat instead of stopping the playhead: what
        was already scheduled stays scheduled, and the server's frozen queue
        holds it.

        The client's reaction does not have to be precise. Between the server's
        stop and this call, a little look-ahead has already gone out; it lands
        in the server's frozen queue and fires on resume in its exact relative
        place. The exactness is the engine's, not the client's.

        Idempotent: freezing an already frozen clock keeps the first freeze's
        position."""
        if self._frozen_at is None:
            self._frozen_at = self._now()
        return self

    def thaw(self):
        """Resume from where `freeze` left the beat.

        The pacing origin shifts by the time spent frozen, so those seconds are
        not part of the piece: the beat picks up where it stopped rather than
        jumping forward by the length of the pause."""
        if self._frozen_at is not None:
            if self._mono_start is not None:
                self._mono_start += self._now() - self._frozen_at
            self._frozen_at = None
        return self

    @property
    def start_time(self):
        """Wall-clock origin (Unix seconds) of the current beat axis — the
        instant beat 0 falls on — or ``None`` before the first `start`. The
        Server uses it to turn a logical beat into a wall-clock OSC timetag:
        the **wall** clock, kept separate from the monotonic pacing source so
        timetags stay valid Unix time. A `stop` leaves it in place (it is the
        axis a later `start` resumes); a `start` re-places it so the held beat
        keeps mapping to now."""
        return self._unix_start

    @property
    def pacing_origin(self):
        """The timebase value (seconds) of the current beat axis' zero, placed
        by `start`. For a sample-clock timebase this is
        ``sample_origin / sample_rate``, which the Server turns into the
        absolute sample for ``/sched_at``."""
        return self._mono_start

    def _gesture_at(self):
        """The beat a tempo gesture with no ``at`` is written at.

        Inside a routine **on this clock**, the routine's own logical beat: the
        yield-exact instant `Moment` already stamps on everything that wake
        emits, so a tempo change made beside a note is written where the note
        is. Anywhere else -- from the main thread, from another clock's routine
        -- the clock's current beat, which is what "now" means there.
        """
        from .main import main

        routine = main.current_routine
        beat = getattr(routine, "_logical_beat", None)
        if beat is not None and getattr(routine, "clock", None) is self:
            return float(beat)
        return self.beats()

    def set_tempo(self, tempo, over=None, unit="beats", curve="linear", at=None):
        """**The tempo gesture**, from the beat the clock is on.

        With no ``over`` it is a **step**: the tempo changes from here, pinning
        the current instant, so the beat the clock is on keeps mapping to the
        second it already mapped to and nothing already scheduled jumps.

        With ``over`` it is a **shape written over a stretch** — an accelerando
        or a ritardando reaching ``tempo`` and holding it. And ``tempo`` may be
        an `Env` (or any object with ``levels``, ``times`` and ``curves``), in
        which case the whole envelope is written in one call and ``over`` is
        not needed: its own times are the extents.

        Args:
            tempo: the tempo to reach, in beats per second — or an envelope of
                them, which must be of **finite duration** (no sustain and no
                loop: those are a gate's ideas, and a piece's tempo has no
                gate).
            over: how far the change is spread. ``None`` is the step.
            unit: what ``over`` (or an envelope's times) measures —
                ``"beats"``, or ``"seconds"`` (``"secs"``). In seconds the width
                in beats is solved exactly, so an accelerando can be asked for
                by how long it lasts rather than by how many beats it covers.
            curve: the shape — ``"linear"`` (``"lin"``), ``"exponential"``
                (``"exp"``) or a numeric curvature (0 is linear, positive starts
                slow, negative starts fast). An envelope carries its own and
                this is ignored.
            at: the beat to write at. ``None`` is *here* — see below, which is
                not quite the same as `beats`.

        A change is **recorded** rather than overwriting what came before, so
        the beats before it stay convertible afterwards.

        **Where "here" is.** With no ``at``, a gesture made from inside a
        routine on this clock is written at the routine's own **logical** beat —
        the yield-exact instant every event of that wake already shares — and
        anywhere else at `beats`. The two differ by however far the driver has
        paced past the wake, which is inaudible and is not nothing: it is what
        writes a breakpoint at 3.00034 instead of 3, and a map that will be
        **saved as the piece's tempo** carries that forever. Pass ``at`` to say
        where explicitly, in beats, which is also how a tempo is written for a
        piece before any clock has run.

        **Against a map that was written ahead of the clock** — a piece's tempo
        track, a shared map, anything with breakpoints still in front of the
        playhead — the gesture says *from here on*, and what was planned after
        this beat is dropped. That is what a live change means: the past is
        untouched and stays convertible, and the future is the one being played
        now. A rehearsal that must not rewrite the piece runs on
        ``clock.map = piece.map.copy()`` — adopting is authoring, forking is
        performing.
        """
        at = self._gesture_at() if at is None else float(at)
        # A gesture is anchored where it is written, so anything the map still
        # holds past that beat is a plan this gesture replaces. The map itself
        # stays append-only -- refusing to go backwards is right for a value;
        # saying "from here on" is the *gesture's* job, and it is the one thing
        # that lets an RT change land on an NRT-written map.
        self._map.truncate_from(at)
        levels = getattr(tempo, "levels", None)
        if levels is not None:
            self._write_env(at, tempo, unit)
        elif over is None:
            self._map.push(at, float(tempo))
        else:
            # The tempo the shape departs from is the one sounding at `at`, not
            # the affine cache's, which is the last segment's and would be a
            # shape's destination.
            self._map.env(at, [self._map.tempo_at(at), float(tempo)],
                          [float(over)], curve, unit)
        self._sync_map(wake=True)
        return self

    def _write_env(self, at: float, env, unit):
        """An `Env` as a tempo envelope. Its levels are tempos and its times are
        extents in ``unit``; a sustain or a loop point is refused rather than
        ignored, because an envelope of tempo has no gate to hold."""
        if getattr(env, "release_node", None) is not None or \
                getattr(env, "loop_node", None) is not None:
            raise ValueError(
                "a tempo envelope is of finite duration: it has no gate, so a "
                "release_node or a loop_node has nothing to mean"
            )
        self._map.env(at, env.levels, env.times, list(env.curves), unit)

    def bar(self, quant: float, beats: float | None = None) -> float:
        """The bar index the clock's current beat (or an explicit ``beats``
        position) falls in on a grid of ``quant`` beats per bar (0-based;
        ``quant <= 0`` -> 0). The read complement of the ``quant`` argument
        `play` takes — computed in the native core, so a GUI ruler in beats
        shows the same bar:beat this returns."""
        pos = self.beats() if beats is None else beats
        return _native.bar(pos, quant)

    def beat_in_bar(self, quant: float, beats: float | None = None) -> float:
        """The beat within its bar for the clock's current beat (or an
        explicit ``beats`` position) on a grid of ``quant`` beats per bar
        (0-based, in ``[0, quant)``; ``quant <= 0`` returns the position)."""
        pos = self.beats() if beats is None else beats
        return _native.beat_in_bar(pos, quant)

    # ---- master-clock lock (sample timebase) ----

    def lock_to(self, server, warmup: bool = True, timeout: float = 2.0):
        """Lock this clock to a master ``server``'s sample clock, so events
        schedule on the server's own sample axis (drift-free) instead of a
        wall-clock OSC timetag.

        Opt-in: a plain clock paces against wall-clock OSC time, which works
        standalone, against another program, or across a network. `lock_to`
        switches it to the server's sample clock — over UDP it tracks the
        server's published `/clock_query` anchor on its own socket; an in-process
        embedded server needs no tracker at all (the counter is read directly
        from shared memory). The switch is **graceful**: an offline (score)
        server, or a master that does not answer, leaves the clock on
        wall-clock time, so a client with no Clausters server keeps working.
        Returns ``self``.

        **Blocking — call it before `start`/`run`, never from inside a
        routine** (it does `/clock_query` round trips). Release it with `unlock` or
        `close`. **Idempotent**: on an already sample-locked clock it is a no-op
        (keeps the live tracker), so it is safe to call after a
        `Session.live()`/`embed()` that already anchored by default.
        """
        # Idempotent: already sample-locked (e.g. the session auto-locked on
        # creation and the caller also calls lock_to_server) -> keep the live
        # tracker instead of building a second one and leaking the first.
        if self._sample_clock is not None:
            return self
        # An offline (score) destination has no live clock to lock to.
        if getattr(getattr(server, "interface", None), "time_mode", "unix") == "score":
            return self
        # The server owns one reader and hands the same one to every clock: a
        # second model of one counter is another socket and another thread
        # re-anchoring the same number, not a second opinion.
        sc = server.sample_clock(timeout=timeout)
        if not sc.tracking:
            # Fresh reader: probe it, firm it up, and start its loop. Already
            # tracking means another clock paid for all three.
            try:
                sc.anchor()       # one round trip: detect a reachable master
            except (TimeoutError, OSError, RuntimeError):
                # Graceful: no master -> stay on wall clock, and take the dead
                # reader off the server so the next clock probes a fresh one.
                server.release_sample_clock()
                return self
            if warmup:
                sc.warmup(n=4)    # firm up the model before scheduling
            sc.track()
        self._sample_clock = sc
        self.timebase = sc.timebase()
        self._now = self.timebase
        return self

    def unlock(self):
        """Undo a `lock_to`: let go of the server's sample-clock reader and
        return to wall-clock OSC time. Returns ``self``.

        It lets go rather than closes. The reader belongs to the **server** and
        is shared by every clock locked to it, so closing it here would stop
        the others dead; `Server.close` is what releases it."""
        if self._sample_clock is not None:
            self._sample_clock = None
        self.timebase = MonotonicTimebase()
        self._now = self.timebase
        return self

    def close(self):
        """Stop the clock and let go of the server's sample-clock reader, if it
        was locked to one (the reader itself is the server's, and outlives
        this)."""
        self.stop()
        self.unlock()

    # ---- shared transport (phase alignment) ----

    def join_transport(self, server):
        """Adopt a master ``server``'s shared `/transport_set` beat grid as this
        clock's tempo and grid, so a `quant`-ed routine starts on the **same**
        beat as every other client joined to it.

        Reads the transport once; if the server has none defined, the clock
        keeps its own grid (no-op). A sample-locked clock (`lock_to`) aligns
        **sample-exactly**; a plain wall-clock clock aligns to beats through the
        server's OSC-time anchor (drift-bounded). Returns ``self``.

        **Blocking — call it before `start`/`run`, never from a routine.**
        """
        info = server.transport()
        if info is None:
            return self
        origin_sample, tempo = info
        # The shared grid is affine by construction (`/transport_set` is an
        # origin and one tempo), so joining one *declares the piece affine*: the
        # map is replaced by that single segment rather than gaining a
        # breakpoint. A piece with a tempo curve phase-aligns by sample instead.
        self._map = _native.TempoMap(float(tempo))
        self._sync_map(wake=True)
        if isinstance(self.timebase, SampleClockTimebase):
            self._transport = ("sample", float(origin_sample), tempo)
        else:
            # Map the sample-defined origin to OSC time via the /clock_query anchor,
            # so a wall-clock client quantizes on the same grid (the offset is
            # the core's samples->seconds conversion, shared with the server).
            _, args = server.request("/clock_query", expect=("/clock_query.reply",))
            sample0, rate, osc0 = int(args[0]), float(args[1]), float(args[2])
            origin_osc = osc0 + _native.samples_to_secs(int(origin_sample) - sample0, rate)
            self._transport = ("wall", origin_osc, tempo)
        return self

    def leave_transport(self):
        """Stop following a joined transport; `quant` returns to the clock's own
        grid. Returns ``self``."""
        self._transport = None
        return self

    @property
    def joined(self) -> bool:
        """Whether this clock is following a shared transport grid
        (`join_transport`)."""
        return self._transport is not None

    def grid_beat(self) -> float:
        """Current position, in beats, on the grid `quant` snaps to: the shared
        transport grid when joined, else the clock's own elapsed beats.

        The two are different axes on purpose. The clock's own beat starts when
        *it* starts; the shared one is the conductor's, running whether this
        client is playing or not — which is what makes two clients started
        seconds apart agree on where the next bar falls."""
        if self._transport is None:
            return self.beats()
        kind, origin, tempo = self._transport
        if kind == "sample":
            now = self.timebase.current_sample()
            return (now - origin) * tempo / self.timebase.sample_rate
        return (time.time() - origin) * tempo

    def _quant_delay(self, quant) -> float:
        """Beats to wait so a routine starts on the next ``quant`` boundary of
        the grid (``None``/``0`` -> now; the snapping rule is the core's
        ``quant_delay``, shared by every client)."""
        if not quant:
            return 0.0
        return _native.quant_delay(self.grid_beat(), quant)

    # ---- scheduling ----

    def _push(self, beat: float, item):
        key = id(item)
        entry = self._items.get(key)
        if entry is None:
            self._items[key] = [item, 1]
        else:
            entry[1] += 1
        self._queue.push(beat, key)

    def _take(self, key):
        """The item for a popped ``key``, dropping the strong reference once no
        queued entry needs it."""
        entry = self._items[key]
        entry[1] -= 1
        if entry[1] == 0:
            del self._items[key]
        return entry[0]

    def _watch_for_an_undriven_clock(self):
        """Arms the exit warning the first time something is queued on a clock
        nobody has driven yet.

        Queueing before the drive starts is the **normal** shape — an offline
        score is built and then `render`ed, and a live one may be scheduled and
        then `start`ed — so there is nothing to say at `sched` time. What is
        always a mistake is reaching the end of the program with a queue and no
        drive: the routines never ran, no exception was raised and nothing was
        logged, which is indistinguishable from silence.

        Only a **session's** clock is watched. That is the one a score is
        played onto (`Routine(f).play(session.clock)`), and the one whose
        lifecycle ends with the program; a bare `TempoClock` belongs to
        whoever built it — a transport, a test, another library object — and
        leaving items on its queue is that owner's business."""
        if self._driven or self._exit_hook or self.session is None:
            return
        self._exit_hook = True
        atexit.register(self._warn_if_undriven)

    def _warn_if_undriven(self):
        if self._driven or self._queue.peek_time() is None:
            return
        print(
            "clausters: this program ends with routines queued on a clock that was "
            "never started — `Routine(f).play(clock)` only schedules; a session runs "
            "them with session.start(), session.run(seconds) or, offline, "
            "session.render()",
            file=sys.stderr,
        )

    def sched(self, delay_beats: float, item):
        """Schedule ``item`` to run ``delay_beats`` from the current beat.

        ``item`` is a `Routine` (or any `Stream`), or a plain callable for a
        one-shot. When resumed, a routine is rescheduled by whatever delay it
        yields; a callable that returns a number is rescheduled by that number,
        and one returning ``None`` runs once. Safe to call from another thread
        or from inside a running routine.
        """
        with self._cond:
            self._push(self.beats() + delay_beats, item)
            self._cond.notify()
        self._watch_for_an_undriven_clock()

    def sched_abs(self, beat: float, item):
        """Schedule ``item`` at an absolute ``beat``, rather than relative to
        the current beat as `sched` does."""
        with self._cond:
            self._push(beat, item)
            self._cond.notify()
        self._watch_for_an_undriven_clock()

    def play(self, routine, quant=None):
        """Schedule a routine (or callable), snapping its start to a beat grid.

        Args:
            routine: a `Routine`, any `Stream`, or a one-shot callable.
            quant: start quantization -- the routine starts on the next beat
                that is a multiple of ``quant`` (e.g. ``4`` = the next bar in
                4/4). ``None`` or ``0`` starts immediately. The grid is the
                clock's own elapsed beats, or a shared one when the clock has
                joined a transport (`join_transport`); for multi-client
                alignment start the clock before playing the quantized routine.
        """
        self.sched(self._quant_delay(quant), routine)
        return routine

    def clear(self):
        """Drop every item currently in the schedule queue."""
        with self._cond:
            self._queue.clear()
            self._items.clear()

    def unsched(self, item):
        """Remove a specific scheduled ``item`` from the queue (by identity),
        leaving the rest in order. Used to cancel one routine — e.g. a
        `clausters.seq.timeline.Playhead` stopping or seeking — without clearing
        everything else `clear` would drop."""
        with self._cond:
            key = id(item)
            if key in self._items:
                removed = self._queue.remove(key)
                entry = self._items[key]
                entry[1] -= removed
                if entry[1] <= 0:
                    del self._items[key]
            self._cond.notify()

    # ---- driving ----

    def _wake(self, item, beat):
        """Resume ``item`` at ``beat``; reschedule if it asks for more time.

        The resumption itself is `clausters.base.stream.resume`, shared with the
        `clausters.base.appclock.AppClock` -- what is this clock's is the beat
        the routine is woken on and the requeue."""
        delta = resume(item, self, logical=beat)
        if delta is not None:
            with self._cond:
                self._push(beat + float(delta), item)
                self._cond.notify()

    def render(self, until_beat: float | None = None, max_steps: int | None = None):
        """NRT drive: process the queue in beat order without sleeping.

        Returns when the queue is empty (or the next event is past
        ``until_beat``). Whatever the routines emit (through a Server) lands in
        that Server's interface — here we only advance time and resume them.

        ``max_steps`` bounds the number of **resumes**, raising once it is
        passed. It defaults to no bound, which is the right default: a long
        offline render of a real score is meant to run for a long time. It is
        for the caller who knows its source might never end — a bounce of an
        endless pattern (`clausters.seq.Timeline.from_pattern`) — because a
        routine cannot report that itself: a routine that raises loses its own
        place and nothing else (see `_wake`), so a guard inside one is
        swallowed by design."""
        self._mode = "nrt"
        self._driven = True
        self._logical_beat = 0.0
        steps = 0
        try:
            while True:
                beat = self._queue.peek_time()
                if beat is None or (until_beat is not None and beat > until_beat):
                    break
                steps += 1
                if max_steps is not None and steps > max_steps:
                    raise RuntimeError(
                        f"render: still going after {max_steps} resumes — "
                        f"the source does not end on its own"
                    )
                _, key = self._queue.pop_due(beat)
                self._logical_beat = beat
                self._wake(self._take(key), beat)
        finally:
            self._mode = "stopped"
        return self

    def start(self):
        """Begin the real-time driver on a background thread.

        A restart **resumes** at the beat `stop` left the clock on, so what is
        still queued keeps its place in the music: `stop`/`start` is a
        transport, not a reset (`clear` is the reset). Both origins are placed
        accordingly — a beat's position in seconds is measured from the clock's
        own zero, so resuming at beat *b* puts the origins ``beats2secs(b)``
        seconds in the past."""
        if self._running:
            return self
        self._mode = "rt"
        self._driven = True
        self._running = True
        held = self.beats2secs(self._logical_beat)
        self._mono_start = self._now() - held    # pacing origin (monotonic)
        self._unix_start = time.time() - held    # wall-clock origin (timetags)
        self._thread = threading.Thread(target=self._run_rt, name="TempoClock", daemon=True)
        self._thread.start()
        return self

    def stop(self):
        """Stop the real-time driver and join its background thread; returns
        ``self``.

        The beat the clock reached is **held**: `beats` keeps reporting it
        while stopped, and a later `start` resumes from it. What is queued
        stays queued (`clear` drops it)."""
        with self._cond:
            # Freeze the beat first: from here `beats()` reports it, because
            # the clock is no longer running. The two origins are deliberately
            # *not* cleared — `_wake` runs outside this lock, and a Server
            # emitting there reads them; they stay the correct origins of the
            # beat axis a later `start` resumes.
            self._logical_beat = self.beats()
            self._running = False
            self._cond.notify_all()
        if self._thread is not None:
            self._thread.join(timeout=1.0)
            self._thread = None
        self._mode = "stopped"
        return self

    def run(self, seconds: float):
        """Convenience: run the RT driver for ``seconds`` then stop."""
        self.start()
        time.sleep(seconds)
        return self.stop()

    def _run_rt(self):
        while True:
            with self._cond:
                if not self._running:
                    break
                beat = self._queue.peek_time()
                if beat is None:
                    self._cond.wait(timeout=0.05)
                    continue
                wait = self.beats2secs(beat) - (self._now() - self._mono_start)
                if wait > 0.0:
                    self._cond.wait(timeout=wait)
                    continue
                _, key = self._queue.pop_due(beat)
                item = self._take(key)
            # Outside the lock: emitting/sending must not block the queue.
            self._wake(item, beat)
