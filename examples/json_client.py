#!/usr/bin/env python3
"""Example Python client: generates defs as JSON and drives Clausters over
OSC. Standard library only — no python-osc needed.

Start a server first:

    cargo run --release                    # UGen defs only
    cargo run --release --features faust   # + Faust defs

then run one or more demos (default: status):

    python3 examples/json_client.py status ugen faust quit

`ugen`   defines an amplitude-modulated noise synth via /d_recv and plays it.
`faust`  builds two Faust defs as JSON box trees via /d_faust (needs the
         feature): a sine from primitives and one importing the Faust stdlib.
`wavetable` computes a 256-point table in Python and plays it through
         `waveform` + `rdtable` (needs the faust feature).
`buffer` writes a WAV, loads it with /b_allocRead, plays it with PlayBuf at
         the file's pitch (rate from /b_info and /status), then frees it.
`disk`   records a sine straight to a WAV with DiskOut, then streams it back
         from disk with DiskIn (real-time disk I/O, no buffer load).
`bundle` schedules a melody in advance with NTP-timetagged bundles: the
         server fires each note sample-accurately on its own clock.
`feedback` plays a resonant comb built with LocalIn/LocalOut: the graph's
         one-block feedback delay makes a one-channel loop ring at
         sampleRate/64 (≈ 750 Hz at 48 kHz).
`signal` builds Faust defs with the **Signal API** (`{"signals": […]}`): a
         sine from an explicit `recursion`/`self` phasor, and a one-pole
         lowpass on noise — explicit sample-accurate feedback (needs the
         faust feature).
"""

import json
import math
import os
import socket
import struct
import sys
import time
import wave

SERVER = ("127.0.0.1", 57110)

# ---- minimal OSC 1.0 encoding/decoding (int32, float32, string, blob) ----


def _pad(data: bytes) -> bytes:
    return data + b"\x00" * (-len(data) % 4)


def _string(s: str) -> bytes:
    return _pad(s.encode() + b"\x00")


class Int64:
    """Marker for an OSC int64 (`h`) argument — `/sched` sample targets."""

    def __init__(self, value: int):
        self.value = int(value)


def message(addr: str, *args) -> bytes:
    tags, data = ",", b""
    for a in args:
        if isinstance(a, bool):
            raise TypeError("OSC has no bool tag here; use int")
        if isinstance(a, Int64):
            tags, data = tags + "h", data + struct.pack(">q", a.value)
        elif isinstance(a, int):
            tags, data = tags + "i", data + struct.pack(">i", a)
        elif isinstance(a, float):
            tags, data = tags + "f", data + struct.pack(">f", a)
        elif isinstance(a, str):
            tags, data = tags + "s", data + _string(a)
        elif isinstance(a, bytes):
            tags, data = tags + "b", data + struct.pack(">i", len(a)) + _pad(a)
        else:
            raise TypeError(f"unsupported OSC argument: {a!r}")
    return _string(addr) + _string(tags) + data


NTP_UNIX_OFFSET = 2_208_988_800


def bundle(seconds_ahead: float, *packets: bytes) -> bytes:
    """An OSC bundle timetagged `seconds_ahead` from now."""
    target = time.time() + seconds_ahead + NTP_UNIX_OFFSET
    tag = struct.pack(">II", int(target), int((target % 1.0) * 2**32))
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + tag + body


def _read_string(data: bytes) -> tuple[str, bytes]:
    end = data.index(b"\x00")
    return data[:end].decode(), data[(end + 4) // 4 * 4:]


def decode(packet: bytes) -> tuple[str, list]:
    addr, rest = _read_string(packet)
    tags, rest = _read_string(rest)
    args = []
    for tag in tags.lstrip(","):
        if tag == "i":
            args.append(struct.unpack(">i", rest[:4])[0])
            rest = rest[4:]
        elif tag == "h":
            args.append(struct.unpack(">q", rest[:8])[0])
            rest = rest[8:]
        elif tag == "f":
            args.append(struct.unpack(">f", rest[:4])[0])
            rest = rest[4:]
        elif tag == "d":
            args.append(struct.unpack(">d", rest[:8])[0])
            rest = rest[8:]
        elif tag == "s":
            value, rest = _read_string(rest)
            args.append(value)
        elif tag == "b":
            size = struct.unpack(">i", rest[:4])[0]
            args.append(rest[4:4 + size])
            rest = rest[4 + (size + 3) // 4 * 4:]
        else:
            raise ValueError(f"unsupported reply tag {tag!r}")
    return addr, args


class Client:
    def __init__(self):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.settimeout(5.0)

    def send(self, addr: str, *args):
        self.sock.sendto(message(addr, *args), SERVER)

    def send_raw(self, packet: bytes):
        self.sock.sendto(packet, SERVER)

    def reply(self, quiet: bool = False) -> tuple[str, list]:
        packet, _ = self.sock.recvfrom(65536)
        addr, args = decode(packet)
        if not quiet:
            print(f"  <- {addr} {args}")
        return addr, args


# ---- /d_recv: SynthDef JSON generation ----


class SynthDefBuilder:
    """Builds the /d_recv wire format; see docs/schemas.md for the spec."""

    def __init__(self, name: str):
        self.name, self.controls, self.ugens = name, [], []

    def control(self, name: str, default: float) -> dict:
        self.controls.append({"name": name, "default": default})
        return {"control": len(self.controls) - 1}

    def add(self, kind: str, *inputs, **static) -> dict:
        """`static` carries non-signal fields (DiskIn/DiskOut `path`, `loop`,
        `format`); most UGens take only signal inputs."""
        resolved = [i if isinstance(i, dict) else {"const": float(i)} for i in inputs]
        self.ugens.append({"kind": kind, "inputs": resolved, **static})
        return {"ugen": len(self.ugens) - 1}

    def blob(self) -> bytes:
        spec = {"name": self.name, "controls": self.controls, "ugens": self.ugens}
        return json.dumps(spec).encode()


def demo_ugen(client: Client):
    """Amplitude-modulated noise: WhiteNoise · (SinOsc(rate)·0.5 + 0.5) · amp."""
    d = SynthDefBuilder("amnoise")
    rate, amp = d.control("rate", 2.0), d.control("amp", 0.2)
    lfo = d.add("Mul", d.add("Add", d.add("Mul", d.add("SinOsc", rate), 0.5), 0.5), amp)
    signal = d.add("Mul", d.add("WhiteNoise"), lfo)
    d.add("Out", 0, signal)
    d.add("Out", 1, signal)

    print("ugen demo: /d_recv amnoise")
    client.send("/d_recv", d.blob())
    client.reply()
    print("  /s_new amnoise 3000 (rate 6)")
    client.send("/s_new", "amnoise", 3000, 1, 0, "rate", 6.0)
    time.sleep(1.5)
    print("  /n_set 3000 rate 1.5")
    client.send("/n_set", 3000, "rate", 1.5)
    time.sleep(1.5)
    client.send("/n_free", 3000)


# ---- /d_faust: JSON → Faust Box API generation ----


def box(op: str, *inputs) -> dict:
    return {"op": op, "in": list(inputs)}


def hslider(label: str, init: float, lo: float, hi: float, step: float) -> dict:
    return {"op": "hslider", "label": label,
            "init": init, "min": lo, "max": hi, "step": step}


def faust(src: str) -> dict:
    """Escape hatch: embeds Faust source (stdlib available) as a box."""
    return {"op": "faust", "src": src}


def sine_def() -> str:
    """sin(2π · phasor(freq)) · amp from primitives, no stdlib."""
    wrap = box("split", "_", box("sub", "_", box("floor", "_")))
    freq = hslider("freq", 440.0, 20.0, 20000.0, 0.01)
    phasor = box("rec", box("seq", box("add", "_", box("div", freq, 48000.0)), wrap), "_")
    sine = box("sin", box("mul", 6.283185307179586, phasor))
    return json.dumps(box("mul", sine, hslider("amp", 0.2, 0.0, 1.0, 0.001)))


def stdlib_def() -> str:
    """The same idea via the stdlib: os.osc embedded with the `faust` op."""
    osc = faust('import("stdfaust.lib"); process = os.osc(220);')
    return json.dumps(box("seq", osc, box("mul", "_", 0.15)))


def wavetable_def() -> str:
    """F5: a wavetable oscillator whose table is *computed in Python* — the
    first four harmonics of a saw — and shipped inside a `waveform` box. No
    Faust source formatting, no server-side files."""
    n = 256
    table = [sum(math.sin(math.tau * k * i / n) / k for k in range(1, 5))
             for i in range(n)]
    peak = max(abs(x) for x in table)
    table = [x / peak for x in table]
    wrap = box("split", "_", box("sub", "_", box("floor", "_")))
    freq = hslider("freq", 220.0, 20.0, 20000.0, 0.01)
    phasor = box("rec", box("seq", box("add", "_", box("div", freq, 48000.0)), wrap), "_")
    idx = box("intcast", box("mul", phasor, n))
    read = {"op": "rdtable", "in": [{"op": "waveform", "values": table}, idx]}
    return json.dumps(box("mul", read, hslider("amp", 0.2, 0.0, 1.0, 0.001)))


def demo_wavetable(client: Client):
    print("wavetable demo: /d_faust jwavetable (table computed client-side)")
    client.send("/d_faust", "jwavetable", wavetable_def())
    addr, _ = client.reply()  # compilation is async: /done or /fail
    if addr == "/fail":
        print("  (is the server running with --features faust?)")
        return
    print("  /s_new jwavetable 3003 (freq 220)")
    client.send("/s_new", "jwavetable", 3003, 1, 0)
    time.sleep(1.5)
    print("  /n_set 3003 freq 330")
    client.send("/n_set", 3003, "freq", 330.0)
    time.sleep(1.5)
    client.send("/n_free", 3003)


def demo_faust(client: Client):
    for name, payload in [("jsine", sine_def()), ("jstdlib", stdlib_def())]:
        print(f"faust demo: /d_faust {name}")
        client.send("/d_faust", name, payload)
        addr, args = client.reply()  # compilation is async: /done or /fail
        if addr == "/fail":
            print("  (is the server running with --features faust?)")
            return

    print("  /s_new jsine 3001 (freq 330)")
    client.send("/s_new", "jsine", 3001, 1, 0, "freq", 330.0)
    time.sleep(1.0)
    print("  /n_set 3001 freq 550")
    client.send("/n_set", 3001, "freq", 550.0)
    time.sleep(1.0)
    print("  /s_new jstdlib 3002 (mixes with jsine)")
    client.send("/s_new", "jstdlib", 3002, 1, 0)
    time.sleep(1.5)
    client.send("/n_free", 3001)
    client.send("/n_free", 3002)


# ---- /b_*: buffers and PlayBuf ----


def write_test_wav(path: str, freq: float, seconds: float, sample_rate: int):
    """A mono int16 sine, written with the stdlib `wave` module."""
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sample_rate)
        n = int(seconds * sample_rate)
        samples = (
            int(0.3 * 32767 * math.sin(2 * math.pi * freq * i / sample_rate))
            for i in range(n)
        )
        w.writeframes(b"".join(struct.pack("<h", s) for s in samples))


def demo_buffer(client: Client):
    path = os.path.join("/tmp", f"clausters_demo_{os.getpid()}.wav")
    write_test_wav(path, freq=330.0, seconds=1.0, sample_rate=22050)

    print(f"buffer demo: /b_allocRead 10 {path}")
    client.send("/b_allocRead", 10, path)
    addr, _ = client.reply()  # async: /done /b_allocRead 10
    if addr == "/fail":
        return
    client.send("/b_query", 10)
    _, info = client.reply()  # /b_info: bufnum, frames, channels, sampleRate
    file_sr = info[3]
    client.send("/status")
    _, status = client.reply()
    server_sr = status[7]

    # PlayBuf's rate is frames per output sample: 1.0 plays at the server
    # rate, so the file's pitch needs file_sr / server_sr.
    d = SynthDefBuilder("bplayer")
    rate = d.control("rate", 1.0)
    d.add("Out", 0, d.add("PlayBuf", 10, 0, rate, 1))
    client.send("/d_recv", d.blob())
    client.reply()

    pitch_true = file_sr / server_sr
    print(f"  /s_new bplayer 3003 (rate {pitch_true:.3f}: the file's pitch)")
    client.send("/s_new", "bplayer", 3003, 1, 0, "rate", pitch_true)
    time.sleep(2.0)
    print("  /n_set 3003 rate ×1.5 (a fifth up)")
    client.send("/n_set", 3003, "rate", pitch_true * 1.5)
    time.sleep(2.0)
    client.send("/n_free", 3003)
    client.send("/b_free", 10)
    client.reply()  # /done /b_free 10
    os.remove(path)


def demo_disk(client: Client):
    """Streaming disk I/O: record a sine straight to a WAV with DiskOut, then
    stream it back from disk with DiskIn (no buffer load). Both stream in real
    time, so the file can be arbitrarily long."""
    path = os.path.join("/tmp", f"clausters_diskdemo_{os.getpid()}.wav")

    # 1. DiskOut: SinOsc(440)*0.2 -> mono float WAV. No Out, so it only records.
    rec = SynthDefBuilder("drec")
    sig = rec.add("Mul", rec.add("SinOsc", 440.0), 0.2)
    rec.add("DiskOut", sig, path=path, format="float")
    client.send("/d_recv", rec.blob())
    client.reply()
    print(f"disk demo: recording 440 Hz to {path} for ~1.2 s (DiskOut)")
    client.send("/s_new", "drec", 3006, 1, 0)
    time.sleep(1.2)
    # Freeing the synth joins the writer thread, which finalizes the WAV.
    client.send("/n_free", 3006)
    time.sleep(0.3)

    # 2. DiskIn: stream the file back (looping) to both outputs.
    play = SynthDefBuilder("dplay")
    streamed = play.add("DiskIn", 0.0, path=path, loop=True)
    play.add("Out", 0, streamed)
    play.add("Out", 1, streamed)
    client.send("/d_recv", play.blob())
    client.reply()
    print("  streaming it back from disk for ~2 s (DiskIn, looping)")
    client.send("/s_new", "dplay", 3007, 1, 0)
    time.sleep(2.0)
    client.send("/n_free", 3007)
    time.sleep(0.2)
    os.remove(path)


# ---- timed bundles (M6) ----


def demo_bundle(client: Client):
    """An arpeggio scheduled entirely up front: every /s_new and /n_free
    travels now inside a timetagged bundle, and the server fires them
    sample-accurately — note the machine-steady rhythm."""
    notes = [330.0, 440.0, 550.0, 660.0, 880.0]
    print(f"bundle demo: scheduling {len(notes)} notes 0.4 s apart")
    for i, freq in enumerate(notes):
        when = 0.5 + i * 0.4
        node = 3100 + i
        on = message("/s_new", "default", node, 1, 0, "freq", freq, "amp", 0.3)
        off = message("/n_free", node)
        client.send_raw(bundle(when, on))
        client.send_raw(bundle(when + 0.3, off))
    time.sleep(0.5 + len(notes) * 0.4 + 0.2)


# ---- /d_faust: the Signal API (JSON signal tree) ----


def sig(op: str, *inputs) -> dict:
    return {"op": op, "in": list(inputs)}


def sig_slider(label: str, init: float, lo: float, hi: float, step: float) -> dict:
    return {"op": "hslider", "label": label,
            "init": init, "min": lo, "max": hi, "step": step}


def signal_sine_def() -> str:
    """sin(2π·phasor(freq))·0.2 via the Signal API: the phasor is the explicit
    recursion `phasor = wrap(self + freq/SR)` (no point-free wires here)."""
    freq = sig_slider("freq", 330.0, 20.0, 20000.0, 0.01)
    acc = sig("add", {"op": "self"}, sig("div", freq, 48000.0))
    phasor = {"op": "recursion", "in": [sig("sub", acc, sig("floor", acc))]}
    sine = sig("sin", sig("mul", 6.283185307179586, phasor))
    return json.dumps({"signals": [sig("mul", sine, 0.2)]})


def signal_lowpass_def() -> str:
    """A one-pole lowpass `y = (1-a)·x + a·y'` reading audio input 0 — the
    `self`/`recursion` feedback is one sample, sample-accurate (a UGen graph
    cannot do this across nodes; see the `feedback` demo's one-block limit)."""
    a = sig_slider("a", 0.9, 0.0, 0.999, 0.001)
    x = {"op": "input", "index": 0}
    body = sig("add", sig("mul", sig("sub", 1.0, a), x), sig("mul", a, {"op": "self"}))
    return json.dumps({"signals": [{"op": "recursion", "in": [body]}]})


def demo_signal(client: Client):
    print("signal demo: /d_faust ssine (Signal API recursion/self sine)")
    client.send("/d_faust", "ssine", signal_sine_def())
    addr, _ = client.reply()
    if addr == "/fail":
        print("  (is the server running with --features faust?)")
        return
    print("  /s_new ssine 3010 (freq 330 -> 440)")
    client.send("/s_new", "ssine", 3010, 1, 0)
    time.sleep(1.2)
    client.send("/n_set", 3010, "freq", 440.0)
    time.sleep(1.0)
    client.send("/n_free", 3010)

    # Explicit-feedback filter: a UGen noise source on bus 4, a Signal-API
    # one-pole reading it via its reserved `in` control.
    print("  one-pole lowpass on noise (explicit feedback filter)")
    noise = SynthDefBuilder("noise4")
    noise.add("Out", 4, noise.add("Mul", noise.add("WhiteNoise"), 0.3))
    client.send("/d_recv", noise.blob())
    client.reply()
    client.send("/d_faust", "siglp", signal_lowpass_def())
    if client.reply()[0] == "/fail":
        return
    client.send("/s_new", "noise4", 3011, 1, 0)
    client.send("/s_new", "siglp", 3012, 1, 0, "in", 4.0, "out", 0.0)
    time.sleep(1.5)
    print("  /n_set 3012 a 0.99 (tighter lowpass)")
    client.send("/n_set", 3012, "a", 0.99)
    time.sleep(1.5)
    client.send("/n_free", 3011)
    client.send("/n_free", 3012)


def demo_feedback(client: Client):
    """A resonant comb from LocalIn/LocalOut feedback. The graph is a DAG, so
    the loop goes through a synth-private feedback bus with one control block
    (64 samples) of delay — a one-channel loop therefore rings at
    sampleRate/64 (≈ 750 Hz at 48 kHz). A quiet 3 Hz impulse train re-excites
    it; the 0.98 feedback gain sets the decay. (LocalIn must come before
    LocalOut — the builder adds them in call order, so it does.)"""
    d = SynthDefBuilder("fbcomb")
    fb = d.add("LocalIn", 0)                          # channel 0, previous block
    exc = d.add("Mul", d.add("Impulse", 3.0), 0.3)   # 3 Hz excitation
    mix = d.add("Add", exc, fb)
    d.add("Out", 0, mix)
    d.add("Out", 1, mix)
    d.add("LocalOut", 0, d.add("Mul", mix, 0.98))    # feed back, decaying

    print("feedback demo: /d_recv fbcomb (LocalIn/LocalOut resonant comb)")
    client.send("/d_recv", d.blob())
    client.reply()
    print("  /s_new fbcomb 3005 — rings at sampleRate/64 ≈ 750 Hz")
    client.send("/s_new", "fbcomb", 3005, 1, 0)
    time.sleep(3.0)
    client.send("/n_free", 3005)


def score_bundle(seconds: float, *packets: bytes) -> bytes:
    """An OSC bundle for an NRT score: the timetag counts seconds from the
    start of the render, not wall-clock time."""
    tag = struct.pack(">II", int(seconds), int((seconds % 1.0) * 2**32))
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + tag + body


def demo_score():
    """Writes a binary score file (the same arpeggio as the bundle demo) for
    the offline renderer; no server needed. Render and listen with:

        ./target/release/clausters --nrt /tmp/clausters_score.osc /tmp/out.wav
        ffplay -autoexit /tmp/out.wav
    """
    path = "/tmp/clausters_score.osc"
    notes = [330.0, 440.0, 550.0, 660.0, 880.0]
    packets = []
    for i, freq in enumerate(notes):
        when = 0.1 + i * 0.4
        node = 3100 + i
        packets.append(
            score_bundle(
                when, message("/s_new", "default", node, 1, 0, "freq", freq, "amp", 0.3)
            )
        )
        packets.append(score_bundle(when + 0.3, message("/n_free", node)))
    # The render ends at the last bundle's time: this one sets the duration.
    packets.append(score_bundle(0.1 + len(notes) * 0.4, message("/n_free", 3100)))
    with open(path, "wb") as f:
        f.write(b"".join(struct.pack(">i", len(p)) + p for p in packets))
    print(f"wrote {path} ({len(packets)} bundles); render it with:")
    print(f"  ./target/release/clausters --nrt {path} /tmp/out.wav")


def main():
    demos = sys.argv[1:] or ["status"]
    client = Client()
    for demo in demos:
        if demo == "status":
            client.send("/status")
            client.reply()
        elif demo == "ugen":
            demo_ugen(client)
        elif demo == "faust":
            demo_faust(client)
        elif demo == "wavetable":
            demo_wavetable(client)
        elif demo == "buffer":
            demo_buffer(client)
        elif demo == "disk":
            demo_disk(client)
        elif demo == "bundle":
            demo_bundle(client)
        elif demo == "feedback":
            demo_feedback(client)
        elif demo == "signal":
            demo_signal(client)
        elif demo == "score":
            demo_score()
        elif demo == "quit":
            client.send("/quit")
            client.reply()
        else:
            sys.exit(
                f"unknown demo: {demo} (use status, ugen, faust, wavetable, buffer, disk, bundle, feedback, signal, score, quit)"
            )


if __name__ == "__main__":
    main()
