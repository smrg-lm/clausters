# Reading the server: buses, taps and takes

Most of what a script reads off the server it *asks* for and gets back at once:
a buffer's samples, a query's reply, a summary built from either. This chapter
is the other kind — what the server keeps **sending**, because what is being
watched changes faster than anything could ask.

| What | Path | For |
|---|---|---|
| a **control bus** | `/bus_stream` → periodic snapshots | meters, read-outs, slow traces |
| an **audio bus** | `/bus_tapStream` → windows of samples | oscilloscopes, phasescopes, spectra |
| a **take as it records** | `/buffer_stream` → the overview as it is written | a picture that grows with the sound |

The GUI host reads all three on its own — that is why a GuiDef naming a bus, a
tap or a take draws with no script at all, and **the drawing is always its**: a
client names what to look at (`plot`, `scope`, a `meter`/`scope`/`waveform`
widget) and the host paints it. What is here is the same three paths opened to
*your* script, for everything else a program does with the data — a read-out, a
decision, a summary it hands on, a test.

Everything lives in the `clausters.data` module:

```python
from clausters.data import BusStream, TapStream, RecordingStream
```

Each stream is its **own** OSC client: it sends its subscription out of a
receiver socket of its own and the reports come back there, on the responder
thread. That is what makes the callbacks work at all — the server handle's reply
path is pulled, so a subscription sent over it would have nobody listening — and
it is also why the server's *one subscription per client* rule costs nothing
here: two streams, or one beside a `server.stream_buses(...)` call you make
yourself, replace nothing. Keep a handler to storing and reading, never a round
trip, which is the rule every `OscFunc` callback follows.

## Control buses

```python
level = Bus.control(server=server)
# ... something writes it: a synth's out_ctl, a knob bound to it, bus.set

buses = BusStream.open(server, [level], period_ms=33)
buses.on_snapshot(lambda values, s: report(values[0]))   # ~30 times a second
```

`values` is an `array('f')` in the order you asked for, always holding the
newest snapshot; `buses.value(level)` reads one by handle and gives `nan` for a
bus this stream does not watch. When the read-out goes away, `buses.stop()`
cancels the subscription — the buses themselves are untouched, since a stream
only ever reads.

The server puts a ceiling on how many buses one subscription may list
(`--max-stream-buses`), and the number is **per carrier**: a snapshot is one
message and is never split across replies, so the same server answers a client
over the shared ring and one over TCP with two different figures. A script that
watches a great deal reads it rather than assuming it —
`server.query_info().max_stream_buses` — and a request over the ceiling is
refused whole, leaving whatever subscription was already there.

A stream is a *latest value*, not a history: a script that needs one keeps it.

```python
history = collections.deque([0.0] * 512, maxlen=512)
buses.on_snapshot(lambda values, s: history.append(values[0]))
```

To *see* the history rather than hold it, name the view instead — a `scope`
widget at `rate="control"` plots a control bus's recent past, and the host keeps
the window.

## Audio buses

A control bus carries one value per block; an analysis needs the samples. A
control bus lives permanently in the server's shared segment, so it can always
be read; an audio bus does not, so the server **records** the ones it is asked
for. You never name the recording: you name the bus.

```python
taps = TapStream.open(server, [bus], frames=2048, period_ms=33)
taps.on_data(lambda bus, w: report(max(abs(s) for s in w.samples)))
```

Opening the stream is what starts the recording, and `taps.stop()` is what ends
it. The server has a finite number of rings to record into (8 by default),
shared with whatever the GUI host is drawing — it counts watchers, so several
views of one bus cost one ring, and a stream that cannot get one fails loudly
rather than falling silent. `frames` is clamped to the carrier's bound and to
half the ring, so a window may come back shorter than asked, and a bus whose
recording has not filled one yet sends nothing at all.

Each window arrives with `end_position` — the total samples ever recorded for
that bus at the window's end — so consecutive windows can be placed on the bus's
own timeline: they overlap or gap by exactly the position delta, never by a
guess about the period.

A stereo pair reads as one interleaved window, which is what a correlation and a
phase figure take:

```python
from clausters._native import correlation, lissajous
from clausters.render import channels

left, right = channels(taps.interleaved(bus, 2), 2)   # adjacent buses b and b+1
points = lissajous(left, right)                       # (x, y) per frame
r = correlation(left, right)                          # -1 … +1, or None
```

**To *see* the bus, name it instead**: `scope(bus)` opens an oscilloscope window
on it, and a `scope` widget in a GuiDef puts one inside a window you compose. The
framing and the trigger that make a periodic signal stand still are the host's,
over the same tap — there is nothing to compute here, and nothing that would
agree with the host by coincidence.

## A take as it records

A recording is the one thing that answers no question. A `RecordBuf` fills a
buffer block by block from the audio thread, which is the one place that must
never send a message, so what the writer publishes instead is how far it has got
— into the server's shared memory, where a peer that maps the segment reads it
directly and everybody else reads nothing. `/buffer_stream` is that reading for
whoever cannot map: the server sends the **overview** of the frames that
appeared, at about a hundredth of the audio's bandwidth.

```python
take = Buffer.alloc(10 * 48000, 1, server=server)
stream = RecordingStream.open(server, [take])
stream.on_report(lambda bufnum, s: print(s.written(bufnum), "frames"))

Synth("record_something", {"buf": take.bufnum}, server=server)
```

Each take's cache is allocated at the buffer's **full length** and empty, so the
axis does not move while it fills; `written` is how far the reports have got,
and past it the cache is the silence the buffer is — read up to it and the two
stay apart, which is what a `waveform`'s `fills` does for the picture.
`stream.peaks(take)` is the cache itself, the same bytes `peaks_cache` builds
from samples, so it goes wherever one of those goes.

Only the overview arrives: inside one bucket there is one figure, so a script
that needs the detail reads the take back with `take.get_samples()` once it is
finished.

**To watch a take fill, name it**: a `waveform` widget with `fills` over the
take's buffer follows the same frontier from the host's side, drawn to where the
writer has got and no further. This class is for a script that wants the summary
itself; and when what you want is the *file* a picture maps rather than the bytes,
`peaks_cache_stream_file` grows one on disk that a `waveform(cache=...)` reads as
it fills.
