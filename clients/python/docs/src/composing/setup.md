# Setup: a session, two instruments, a take

Everything in this section runs in one live session. Open your interpreter
and evaluate the blocks as you go — every later page assumes the names this
one defines.

## The session

```python
from clausters import Session

TEMPO = 2.0     # beats per second (the TempoClock convention): 120 bpm
QUANT = 0.5     # the musical grid the editor will snap drags to: half a beat

# `latency` schedules each event a touch ahead (a wall-clock timetag), so the
# server plays it on time instead of "as soon as possible".
session = Session.live(tempo=TEMPO, latency=0.1)
server = session.server
```

`Session.live` attaches to a running server or boots one for you, and bundles
it with a `TempoClock` at `TEMPO` beats per second ([Sessions](../sessions.md)).
Two derived numbers the editor pages will need:

```python
SR = float(server.options.sample_rate)
BEAT = SR / TEMPO      # timeline samples per beat — the unit bridge, one line
print(SR, BEAT)
```

Start the clock now; routines and playheads run on it, and it costs nothing
while idle:

```python
session.start()
```

## The instrument that plays a buffer

The notes of the piece will use the server's stock `default` def. The audio
take needs an instrument of its own, because of a rule you will meet properly
on the next page: a buffer is *data*, and data sounds through the def named to
play it — a def whose `buf` control takes the buffer number, as a sampler's
does.

```python
from clausters.defs import SynthDef, control, out, play_buf

def sampler(name: str = "take") -> SynthDef:
    """Plays a buffer once. The event frees the synth after its `sustain`,
    which is the clip's length — so the take stops when the clip ends."""
    buf = control("buf", 0.0, "ir")
    amp = control("amp", 0.8, "ir")
    sig = play_buf(buf, 0.0, 1.0, 0.0) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))

sampler().send(server)     # blocks on /done — fine at the top level
```

(`add_synthdef` blocks until the server confirms the def. That is fine here,
in the interpreter; the one place it is forbidden is inside a routine on the
clock thread — see [Routines and clocks](../routines-and-clocks.md).)

## The take

A real audio clip, made the way a composition really makes one: **bounced
offline and loaded from the file**. A buffer is loaded or generated on the
server, never push-filled by the client — so we render two beats of a bass
note to a WAV with an offline session, then ask the live server to read it.

```python
import tempfile
from pathlib import Path
from clausters.seq import Pbind, Pseq

def bounce_take(path: str, beats: float = 2.0) -> str:
    """Render a two-beat bass note offline, straight to a WAV."""
    offline = Session.nrt(tempo=TEMPO)
    offline.play(Pbind(midinote=Pseq([36], 1), dur=beats, legato=1.0, amp=0.3))
    stats = offline.render(sample_rate=SR, channels=1, path=path)
    print(f"bounced {stats.frames} frames -> {path}")
    return path

wav = bounce_take(str(Path(tempfile.mkdtemp(prefix="clausters-")) / "take.wav"))
buf = Buffer.read(wav, server=server).query()   # on the server, shape known
print(buf.bufnum, buf.frames, buf.channels)
```

`Buffer.read` loads the file into a server buffer and returns its handle;
`query_buffer` asks the server for the buffer's shape (frames, channels,
sample rate), which the arrangement and the editor will both read. Note the offline
session is the *same* client code as the live one — only the destination
differs; the last page of this section leans on exactly that.

## Hear it once

Before any model enters the picture, confirm the raw pieces work: one note of
the take, played as a one-event pattern on the session's clock.

```python
session.play(Pbind(instrument="take", buf=float(buf.bufnum),
                   dur=Pseq([2.0], 1), legato=1.0))
```

Two beats of bass. (Timing always rides a clock: an event plays itself with
`play(destination)` from *inside* a routine or a playhead running on a
`TempoClock` — never from the bare interpreter, where there is no logical time
to stamp it with. `session.play` wraps the pattern in exactly such a routine.)
That `play(destination)` seam — an object rendering itself onto a server — is
what the whole arrangement model delegates to, and everything after this page
builds on it.

Next: [Elements: the five primitives](elements.md).
