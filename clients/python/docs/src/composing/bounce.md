# Bouncing: the piece as a file

The last change of state: the whole piece, rendered offline. The promise made
on the rendering page comes due here — RT and NRT are the *same* flattening,
differing only in the destination, so the render is sample-identical to what
you heard. There is no export path; there is `render`, pointed at an offline
session.

For a **self-contained** piece — no buffers or automations prepared on a live
server — the free-standing verb does all of this in one line:
`render(piece, path="piece.wav")` bounces it in an ephemeral offline session
(see [The ambient verbs](../verbs.md)). This piece carries server-bound state,
which is exactly what the rest of this page moves by hand.

## The offline session

```python
offline = Session.nrt(tempo=TEMPO)
sampler().send(offline.server)
drone().send(offline.server)
Buffer.read(wav, server=offline.server)  # the take, on the offline server
```

The offline server needs everything the live one had: the defs, and the take
loaded from its file (in NRT these are *scored* at time 0 — the renderer
compiles and loads before time advances).

## Server-bound state moves explicitly

One category of thing does **not** retarget itself: resources that live *on a
server* — the take's buffer number, and the automation's control buffer and
bus. The model is destination-agnostic; those numbers are not. Two of them
matter here:

- the take was the live server's first buffer, and the offline server's
  `Buffer.read` above allocated its first — the numbers line up because both
  allocators started fresh;
- the sweep was `prepare`d on the *live* server, and the drone's voice
  captured the bus *index* when you built its event. Release the live
  resources, prepare against the offline server (scored at time 0), and
  re-point the voice at the bus the offline pass allocated — then render,
  the same verb, the same tree, an offline destination:

```python
sweep.free(server)                           # return the live buffer and bus
sweep.prepare(offline.server)                # score its buffer + bus at time 0
voice.wraps["freq_bus"] = sweep.bus.index    # the voice reads the offline bus
playhead = song.render(offline.server, offline.clock)
```

## Render and write

```python
samples, frames = offline.render(sample_rate=SR, channels=2)
print(f"rendered {frames} frames ({frames / SR:.2f} s)")
```

```python
import struct, wave

out_path = str(Path(tempfile.mkdtemp(prefix="clausters-")) / "piece.wav")
with wave.open(out_path, "wb") as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(int(SR))
    w.writeframes(b"".join(
        struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767))
        for s in samples))
print(f"wrote {out_path}")
```

Listen to it (`pw-play`, or any player): the piece, with every edit
you made on screen — the moved clips, the trimmed take, the redrawn sweep —
because the offline pass flattened the same model your window was editing.
Both renders converge on the same score; the engine that plays it is
the same engine that played it.

To keep working live afterwards, give the sweep its live resources back the
same way (`sweep.free(offline.server)` is not needed — the offline session is
done — just `sweep.prepare(server)` and re-point `voice.wraps["freq_bus"]`).
And when you are done for the day:

```python
session.close()      # stops the clock, the GUI host and a server it booted
```

## Where to go from here

- The finished, self-contained **script form** of this piece — on-screen
  transport buttons, a poll loop, the same four lanes — is
  `clients/python/examples/gui_composer.py`. Read it next to this section and
  you will recognize every line.
- [Composition: the arrangement and the multitrack editor](../composition.md) — the
  concepts, argued rather than exercised.
- [The glossary](glossary.md) — every term this section used, pinned in one
  place.
- The [API reference](../api.md) for `clausters.form`, `clausters.gui` and
  `clausters.seq`; the wire protocol the editor speaks is in the
  [Clausters server book](https://clausters.readthedocs.io/).
