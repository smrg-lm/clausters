#!/usr/bin/env python3
"""Generate info-vectors.json: the readable line every server record prints.

The records are a client's own reading of a reply, and both clients print one
line per record so a REPL can show a node, a buffer or a def without a helper.
In Python that line is a dataclass' ``__str__``; in TypeScript the records are
interfaces and cannot carry a method, so the line is a free formatter. Same
text either way -- which is what this freezes.

The parsers are already asserted to read one wire alike (`tests/info.test.ts`
walks the same arguments as the Python client's own test), so what is left to
drift is the *wording*: a `%g` that rounds differently, a separator, a plural.
Nothing catches that but the strings side by side, and there is no shared core
under this one -- it is presentation, in two languages, from one description.

The JSON is committed; regenerate with:

    python3 gen-info-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's .venv
has it installed editable.)
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs.info import (  # noqa: E402
    BufferInfo, ControlInfo, DefInfo, NodeInfo, NodeMap, UgenInfo, UgenInput,
)
from clausters.defs.server.options import ServerInfo  # noqa: E402

#: Each case is `[record kind, the record's fields, the line it prints]`. The
#: fields are the TypeScript spelling (camelCase), so the test builds the same
#: record from the same JSON rather than transliterating it by hand.
CASES: list[tuple[str, dict, str]] = []


def case(kind: str, fields: dict, record) -> None:
    CASES.append((kind, fields, str(record)))


# ---- controls: a plain one, a Faust range, a graph port -----------------------
case("control", {"name": "freq", "default": 440.0, "rate": "kr"},
     ControlInfo("freq", 440.0, "kr"))
case("control", {"name": "amp", "default": 0.2, "rate": "kr"},
     ControlInfo("amp", 0.2, "kr"))
case("control", {"name": "trig", "default": 0.0, "rate": "tr"},
     ControlInfo("trig", 0.0, "tr"))
case("control",
     {"name": "cutoff", "default": 800.0, "rate": "kr",
      "min": 20.0, "max": 20000.0, "step": 0.5},
     ControlInfo("cutoff", 800.0, "kr", 20.0, 20000.0, 0.5))
case("control",
     {"name": "range", "default": 1.0, "rate": "kr", "min": 0.0, "max": 2.0},
     ControlInfo("range", 1.0, "kr", 0.0, 2.0))
case("control",
     {"name": "mix", "default": 0.5, "rate": "kr",
      "targets": [{"member": 1, "control": "amp", "mul": 1.0, "add": 0.0},
                  {"member": 2, "control": "pan", "mul": 2.0, "add": -1.0}]},
     ControlInfo("mix", 0.5, "kr",
                 targets=((1, "amp", 1.0, 0.0), (2, "pan", 2.0, -1.0))))

# ---- defs ---------------------------------------------------------------------
case("def",
     {"name": "beep", "family": "synth", "exists": True,
      "controls": [{"name": "freq", "default": 440.0, "rate": "kr"},
                   {"name": "amp", "default": 0.2, "rate": "kr"}]},
     DefInfo("beep", "synth", [ControlInfo("freq", 440.0), ControlInfo("amp", 0.2)]))
case("def", {"name": "empty", "family": "faust", "exists": True, "controls": []},
     DefInfo("empty", "faust", []))
case("def", {"name": "nope", "family": "", "exists": False, "controls": []},
     DefInfo("nope", "", [], False))

# ---- buffers ------------------------------------------------------------------
case("buffer",
     {"bufnum": 0, "frames": 1024, "channels": 2, "sampleRate": 48000.0,
      "exists": True},
     BufferInfo(0, 1024, 2, 48000.0))
case("buffer",
     {"bufnum": 3, "frames": 100, "channels": 1, "sampleRate": 0.0, "exists": True},
     BufferInfo(3, 100, 1, 0.0))
case("buffer",
     {"bufnum": 7, "frames": 0, "channels": 0, "sampleRate": 0.0, "exists": False},
     BufferInfo(7, 0, 0, 0.0, False))

# ---- ugen kinds ---------------------------------------------------------------
case("ugenInput", {"name": "freq", "default": 440.0}, UgenInput("freq", 440.0))
case("ugen",
     {"name": "Sine", "arity": 1, "defaultRate": "ar", "rates": ["ar", "kr"],
      "exec": "normal", "bus": "", "needsPath": False, "opFamily": "",
      "spectral": "", "inputs": [{"name": "freq", "default": 440.0}]},
     UgenInfo("Sine", 1, "ar", ("ar", "kr"), "normal", "", False, "", "",
              [UgenInput("freq", 440.0)]))
case("ugen",
     {"name": "EnvGen", "arity": -1, "defaultRate": "kr", "rates": ["kr", "ar"],
      "exec": "normal", "bus": "", "needsPath": False, "opFamily": "",
      "spectral": "", "inputs": [{"name": "gate", "default": 1.0},
                                 {"name": "levelScale", "default": 1.0}]},
     UgenInfo("EnvGen", -1, "kr", ("kr", "ar"), "normal", "", False, "", "",
              [UgenInput("gate", 1.0), UgenInput("levelScale", 1.0)]))
case("ugen",
     {"name": "Out", "arity": 2, "defaultRate": "ar", "rates": ["ar"],
      "exec": "sink", "bus": "audio", "needsPath": False, "opFamily": "",
      "spectral": "", "inputs": []},
     UgenInfo("Out", 2, "ar", ("ar",), "sink", "audio", False, "", "", []))

# ---- nodes --------------------------------------------------------------------
case("nodeMap", {"control": 0, "bus": 3, "audio": False}, NodeMap(0, 3))
case("nodeMap", {"control": 2, "bus": 8, "audio": True}, NodeMap(2, 8, True))
case("node",
     {"id": 1001, "parent": 1000, "prev": -1, "next": -1, "isGroup": False,
      "exists": True, "head": -1, "tail": -1, "name": "", "defname": "beep",
      "controls": {"freq": 440.0, "amp": 0.2},
      "maps": [{"control": 1, "bus": 3, "audio": False}],
      "reads": "-", "writes": "0"},
     NodeInfo(1001, 1000, is_group=False, defname="beep",
              controls={"freq": 440.0, "amp": 0.2}, maps=[NodeMap(1, 3)],
              writes="0"))
case("node",
     {"id": 1000, "parent": 0, "prev": -1, "next": -1, "isGroup": True,
      "exists": True, "head": 1001, "tail": 1001, "name": "voices",
      "defname": "", "controls": {}, "maps": [], "reads": "-", "writes": "-"},
     NodeInfo(1000, 0, is_group=True, head=1001, tail=1001, name="voices"))
case("node",
     {"id": 100, "parent": 0, "prev": -1, "next": -1, "isGroup": True,
      "exists": True, "head": -1, "tail": -1, "name": "", "defname": "",
      "controls": {}, "maps": [], "reads": "-", "writes": "-"},
     NodeInfo(100, 0, is_group=True))
case("node",
     {"id": 4242, "parent": -1, "prev": -1, "next": -1, "isGroup": False,
      "exists": False, "head": -1, "tail": -1, "name": "", "defname": "",
      "controls": {}, "maps": [], "reads": "-", "writes": "-"},
     NodeInfo(4242, exists=False))

# ---- the server itself ---------------------------------------------------------
SERVER = dict(
    blockSize=64, nominalSampleRate=48000.0, actualSampleRate=48000.0,
    channels=2, inputChannels=0, audioBuses=1024, controlBuses=16384,
    maxNodes=1024, maxBuffers=1024, maxGraphChildren=64, maxUgenInputs=64,
    taps=4, tapFrames=8192, maxFrame=65536, maxStreamBuses=128,
)
case("server", SERVER,
     ServerInfo(block_size=64, nominal_sample_rate=48000.0,
                actual_sample_rate=48000.0, channels=2, input_channels=0,
                audio_buses=1024, control_buses=16384, max_nodes=1024,
                max_buffers=1024, max_graph_children=64, max_ugen_inputs=64,
                taps=4, tap_frames=8192, max_frame=65536, max_stream_buses=128))
DRIFTED = dict(SERVER, actualSampleRate=48001.5, taps=0, tapFrames=0)
case("server", DRIFTED,
     ServerInfo(block_size=64, nominal_sample_rate=48000.0,
                actual_sample_rate=48001.5, channels=2, input_channels=0,
                audio_buses=1024, control_buses=16384, max_nodes=1024,
                max_buffers=1024, max_graph_children=64, max_ugen_inputs=64,
                taps=0, tap_frames=0, max_frame=65536, max_stream_buses=128))


def main() -> None:
    out = pathlib.Path(__file__).with_name("info-vectors.json")
    out.write_text(json.dumps(
        [{"kind": kind, "record": fields, "line": line}
         for kind, fields, line in CASES],
        indent=2) + "\n")
    print(f"wrote {out} ({len(CASES)} records)")


if __name__ == "__main__":
    main()
