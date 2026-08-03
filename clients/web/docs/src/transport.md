# The transport: a shared grid, and a piece that freezes

The server keeps one **transport**: a beat grid several clients phase-align on, plus a DAW-style rolling state (playing, and a song position). It has two intensities, and which one you get depends on a single call.

- With **no group bound** it is an advisory: the server stores the grid and the rolling state, broadcasts every change, and never schedules audio from it. Clients read it and obey it because they choose to.
- With **a group bound** the engine enforces it. `transportStop()` freezes that group and everything under it at the exact sample it lands on, and `transportPlay()` thaws them.

The whole surface is on the `Server`, so it is carrier-agnostic: the same calls drive a native `clausters --ws` server and the in-page engine.

## The grid

The grid is small on purpose: an **origin sample** (where beat 0 falls on the server's sample clock) and a **tempo** in beats per second. Beat `b` is sample `origin + b * rate / tempo`.

```js
await server.setTransport(0, 2.0);   // beat 0 at sample 0, two beats a second
await server.transport();            // { originSample: 0, tempo: 2 }, or null
```

One client is the conductor and defines it; last writer wins, and defining it resets the rolling state to stopped at position 0. The rolling state rides on top:

```js
await server.transportPlay(4.0);   // roll from song-position beat 4
await server.transportLocate(16.0); // seek the position
await server.transportStop();       // halt
```

Every change is pushed to each client registered for notifications (`server.notify(true)`, which `Server.open` does by default) as a `/transport_query.reply`, so a page that wants to react to a conductor can read those off `server.onReply` rather than polling. Reading the whole state at once:

```js
const state = await server.transportState();
// { originSample, tempo, playing, position, group, transportSample }
```

`group` is the governed group or `null`, and `transportSample` is the transport clock — samples elapsed *under the transport*, held while it is stopped. It is the time of the piece, as against the device clock (the taps, the bus streams, `/clock_query`), which never stops. The two are one physical clock and cannot drift apart; they differ only while a governed transport is stopped.

## Freezing a piece

Binding a group is what gives the transport teeth:

```js
const piece = new Group({ server });
await server.setTransport(0, 2.0);
await server.transportGroup(piece);   // null unbinds
await server.transportPlay();
```

A frozen node stays in the tree with its internal state untouched — filters keep their memory, phasors their phase, envelopes their position. So a resume **continues** the sound rather than starting it again. That matters most for material the server *generates*: a def running a stochastic process has nothing to read and no messages arriving, so there is no position to seek to — its position **is** its internal state. Continuing is the only thing a pause can honestly mean for it.

Anything scheduled against a governed node waits out the pause with it and fires on resume in its right relative place, so a look-ahead already in flight is not lost. A bundle is atomic, so one holding a governed message and a live one waits entirely.

Unbinding thaws whatever the transport governed, and so does freeing the group — no frozen subtree is ever left with nobody to resume it.

## The page's half of the pause

A clock's beats come from a timebase that only decides how long to sleep before the next wake. A page whose server froze would keep advancing beats and scheduling material ahead — running away from a piece that is not moving. So the clock freezes too:

```js
await server.transportStop();
clock.freeze();                 // hold the beat where it is
// ...
await server.transportPlay();
clock.thaw();                   // and pick it up there
```

`freeze()` holds the logical beat without stopping the clock, and `thaw()` shifts the pacing origin by the time spent frozen, so those seconds are not part of the piece. Your reaction does not have to be precise: between the server's stop and the call, a little look-ahead has already gone out, and it lands in the server's frozen queue to fire on the resume in its exact relative place. The exactness is the engine's, not the page's.

`examples/transport-freeze.html` freezes a generative texture and resumes it, which is the way to *hear* the difference between continuing and restarting.

## Scheduling on the transport axis

Every sample count on the wire belongs to one of the two clocks, and which one a scheduled bundle rides is decided by where its messages point: a bundle targeting a node at or under the governed group waits out a pause, and everything else fires on the device clock as always. Classification happens once, when the bundle is queued.

A client naming an absolute sample of the *piece* declares that axis:

```js
await server.schedAtTransport(sample, [
  ["/synth_new", "grain", ["i", node], ["i", 0], ["i", piece.id]],
]);
```

The declaration is not there to disambiguate — the classification is deterministic, and a client that bound the group knows which of its nodes are governed. It is there to be **verified**: the server compares it against its own classification and fails when they disagree, rather than playing the bundle in the wrong place. It needs a group bound.

## What is not here yet

The advisory half of the transport is readable and drivable from this client, but not yet *joinable*: `clock.joinTransport(server)`, a `Playhead` that follows the broadcasts, and `session.joinTransport()` are the Python client's and have no counterpart here. Until they land, a page that wants to phase-align reads `server.transport()` and quantizes its own start against the grid.

## See also

- [Routines and clocks](routines-and-clocks.md) — the clock the freeze holds, and the logical time it keeps.
- The **[Python client's book](https://clausters-python.readthedocs.io/)** — the same transport with the joining half, the playhead and the conductor examples.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/transport_set`, `/transport_group` and `/sched_atTransport` on the wire.
