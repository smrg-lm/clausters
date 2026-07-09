#!/usr/bin/env python3
"""The editor-grade waveform and spectrogram: lanes, rulers, selection, playhead.

The two heavy views at audio-editor depth. A stereo phrase is rendered
**offline** (no audio device needed for the render), written as one interleaved
``f32`` file, and shown twice from that single mapped resource (the bulk path —
the samples never ride OSC):

- a ``waveform`` with ``channels=2`` draws **both** channels as stacked lanes
  sharing the time axis (one peak pyramid per channel; pass ``overlay=True``
  for per-color overlaid traces instead), with an adaptive **time ruler**
  underneath (1-2-5 steps, clock-time labels because ``sample_rate`` is given);
- a ``spectrogram`` with ``channels=2`` analyzes each channel separately (one
  STFT lane per channel) and adds a **Hz ruler** along the left edge that
  matches its log frequency axis.

Both views navigate identically: **wheel** zooms toward the cursor (the peak
pyramid cross-fades between detail levels, so zooming never pops),
**Shift+drag** pans, **plain drag selects** — the host draws the translucent
selection band and emits ``/gui_event <id> "selection" <start> <len>`` (in
samples) as you drag, which this script prints; ``r`` resets the view. The
selection can also be set from here (``gui.set(id, sel_start=..., sel_len=...)``),
as can the display (``db_floor``/``db_ceil``/``log_freq``/``colormap``).

The **playhead** closes the loop with the live server: the same render is
loaded into a server buffer and looped by a ``PlayBuf`` synth; the script
anchors each pass with the server's sample clock (``/clock``) and sets
``playhead_at`` on both views, so the orange line tracks what you hear — the
GUI host reads the engine clock from the shared segment with **zero per-frame
messages** (pass ``--shm`` to both server and host; without it the playhead
simply stays hidden).

Start the audio server (from the repo root)::

    cargo run -- --shm /dev/shm/clausters_editor

Start the windowed GUI host on the same segment (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 --shm /dev/shm/clausters_editor -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_editor.py

A window opens with the two editor views over the same stereo phrase; drag to
select (watch the events land here), zoom around, and follow the playhead while
the phrase loops. Close the window to stop. Needs a display and a GPU adapter.
"""

import math
import os
import struct
import sys
import tempfile
import time
import wave

from clausters import Session
from clausters.defs import SynthDef, out, play_buf
from clausters.gui import GuiHost, peaks_cache_file, samples_to_file, spectrogram, waveform, window
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48_000.0


def phrase() -> Pbind:
    """Two bars of an arpeggio with amplitude jitter — enough spectral motion
    for the spectrogram to be worth looking at."""
    return Pbind(degree=Pseq([0, 4, 7, 11, 7, 4], repeats=4), dur=0.25,
                 amp=Pwhite(0.1, 0.25))


def render_stereo() -> list:
    """Renders the phrase offline; returns the interleaved stereo f32 frames."""
    session = Session.nrt(tempo=2.0)
    session.play(phrase())
    samples, frames = session.render(sample_rate=SR, channels=2)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) offline")
    return list(samples)


def write_wav(inter: list, path: str):
    """The same interleaved render as a 16-bit stereo WAV, for /b_allocRead."""
    with wave.open(path, "w") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(
            struct.pack("<h", int(32767 * max(-1.0, min(1.0, s)))) for s in inter))


def scene(raw_path: str) -> dict:
    """The editor pair over one mapped stereo file. The `waveform` maps the raw
    samples (and caches its peak pyramids beside them); the `spectrogram`
    analyzes the same file into one STFT lane per channel."""
    return window(
        waveform(10, path=raw_path, channels=2, sample_rate=SR),
        spectrogram(11, path=raw_path, channels=2, sample_rate=SR,
                    window_size=1024, db_floor=-90.0),
        title="Editor: waveform + spectrogram", w=960, h=640, layout="col",
    )


def main():
    tmp = tempfile.mkdtemp(prefix="clausters_editor_")
    raw_path = os.path.join(tmp, "phrase.f32")
    wav_path = os.path.join(tmp, "phrase.wav")
    try:
        inter = render_stereo()
        frames = len(inter) // 2
        seconds = frames / SR
        samples_to_file(inter, raw_path)
        # Not strictly needed (the host builds a sibling cache when it maps the
        # raw file), but shows the multichannel cache built client-side through
        # the shared core — byte-identical to the host's own.
        cache = peaks_cache_file(inter, os.path.join(tmp, "phrase.peaks"), channels=2)
        print(f"wrote {os.path.getsize(raw_path)} B raw, "
              f"{os.path.getsize(cache)} B multichannel peak cache")

        with Session.live() as session:  # UDP to 127.0.0.1:57110
            server = session.server
            # The playhead's sound source: the render in a server buffer.
            write_wav(inter, wav_path)
            bufnum = server.buffers.alloc()
            server.send_msg("/b_allocRead", bufnum, wav_path)
            server.add_synthdef(SynthDef(
                "sampler",
                out(0.0, play_buf(float(bufnum), 0.0)),
                out(1.0, play_buf(float(bufnum), 1.0)),
            ))
            server.sync()

            with GuiHost() as gui:  # 127.0.0.1:57210 by default
                gui.define(1, scene(raw_path))
                # Pre-select the second half from the script; dragging on either
                # view replaces it and reports back as "selection" events.
                gui.set(10, sel_start=float(frames // 2), sel_len=float(frames // 4))
                print("drag to select, Shift+drag to pan, wheel to zoom, r to reset")

                synth = None
                next_pass = 0.0
                start = time.monotonic()
                while time.monotonic() - start < 40.0:
                    now = time.monotonic()
                    if now >= next_pass:
                        # (Re)start a pass and anchor the playhead: the /clock
                        # sample at which buffer position 0 starts sounding.
                        if synth is not None:
                            server.free(synth)
                        _, args = server.request("/clock", expect=("/clock.reply",))
                        clock_samples = float(args[0])
                        synth = server.synth("sampler")
                        gui.set(10, playhead_at=clock_samples)
                        gui.set(11, playhead_at=clock_samples)
                        next_pass = now + seconds + 0.5
                    msg = gui.poll(timeout=0.05)
                    if msg is None:
                        continue
                    addr, args = msg[0], msg[1:]
                    if addr == "/gui_closed":
                        print("window closed")
                        break
                    if addr == "/gui_event" and len(args) >= 4 and args[1] == "selection":
                        wid, _, sel_start, sel_len = args[:4]
                        print(f"widget {wid}: selection {sel_start:.0f} +{sel_len:.0f} samples "
                              f"({sel_start / SR:.3f}s +{sel_len / SR:.3f}s)")
                if synth is not None:
                    server.free(synth)
            server.free_buffer(bufnum)
    finally:
        for name in os.listdir(tmp):
            os.remove(os.path.join(tmp, name))
        os.rmdir(tmp)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
