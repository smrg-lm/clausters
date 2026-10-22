#!/usr/bin/env python3
"""Generate audio-trace-vectors.json: two scripts of actions over an audio
editor, and what the coordinator answered for each -- one where every
service answers, one where a server refuses and is slow.

Every action of an audio editor runs in the shared crate's coordinator and
comes back as ordered effects and awaits, which a client carries out. Two
clients that call it differently -- a fact left out, an await answered
another way, an action that skips the door -- answer different traces even
when the windows look alike. The Python client runs the script here
(`test_gui_audio.audio_trace`) and the trace is frozen; `gui-audio.test.ts`
runs the same script through the web client and compares.

The JSON is committed; regenerate with:

    python3 gen-audio-trace-vectors.py
"""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))
sys.path.insert(0, str(ROOT / "python" / "tests"))

import pytest  # noqa: E402

from test_gui_audio import TRACE_VECTORS, audio_fault_trace, audio_trace  # noqa: E402


def main() -> None:
    with pytest.MonkeyPatch.context() as monkeypatch:
        trace = audio_trace(monkeypatch)
    with pytest.MonkeyPatch.context() as monkeypatch:
        faults = audio_fault_trace(monkeypatch)
    TRACE_VECTORS.write_text(json.dumps({"trace": trace, "faults": faults}, indent=1) + "\n")
    print(f"wrote {TRACE_VECTORS.name}: {len(trace)} actions, {len(faults)} with faults")


if __name__ == "__main__":
    main()
