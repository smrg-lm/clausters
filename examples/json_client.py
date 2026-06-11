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
`buffer` writes a WAV, loads it with /b_allocRead, plays it with PlayBuf at
         the file's pitch (rate from /b_info and /status), then frees it.
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


def message(addr: str, *args) -> bytes:
    tags, data = ",", b""
    for a in args:
        if isinstance(a, bool):
            raise TypeError("OSC has no bool tag here; use int")
        if isinstance(a, int):
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

    def reply(self) -> tuple[str, list]:
        packet, _ = self.sock.recvfrom(65536)
        addr, args = decode(packet)
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

    def add(self, kind: str, *inputs) -> dict:
        resolved = [i if isinstance(i, dict) else {"const": float(i)} for i in inputs]
        self.ugens.append({"kind": kind, "inputs": resolved})
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
        elif demo == "buffer":
            demo_buffer(client)
        elif demo == "quit":
            client.send("/quit")
            client.reply()
        else:
            sys.exit(f"unknown demo: {demo} (use status, ugen, faust, buffer, quit)")


if __name__ == "__main__":
    main()
