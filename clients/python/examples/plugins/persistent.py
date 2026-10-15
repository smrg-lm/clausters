#!/usr/bin/env python3
"""One persistent native instrument, live notes and a server-owned event lane.

The installed Linux package includes the CLAP/VST3 helper; in a checkout run
`scripts/refresh-bin.sh` first. Name an installed instrument with two
variables -- its binary and its native ID, as `server.scan_plugins(binary)`
lists them -- and run:

    PLUGIN_BINARY=/path/instrument.clap PLUGIN_ID=vendor.instrument \
        python clients/python/examples/plugins/persistent.py

It needs an instrument with a native note-expression port and an audible
initial preset, such as Surge XT; an effects collection has none. A sampler
also needs samples in its initial state. The numeric repository fixture can
verify events but is not a musical preset. This is a `# %%` notebook: the same
calls and event sequence as the web page. Python can boot a native process;
the page attaches to one started separately.
"""
# %%
import os
import sys
from clausters import Group, PluginDef, Routine, Session, Synth

BINARY = os.environ.get("PLUGIN_BINARY", "")
PLUGIN_ID = os.environ.get("PLUGIN_ID", "")
if not BINARY or not PLUGIN_ID:
    raise ValueError("set PLUGIN_BINARY and PLUGIN_ID to a native instrument")

session = Session.live(port=57113).activate()
session.clock.set_tempo(2.0)
server = session.server

# %%
rows = server.scan_plugins(BINARY, timeout=60.0)
info = next(row["plugin"] for row in rows
            if row["kind"] == "plugin" and row["plugin"]["id"] == PLUGIN_ID)
if not info["note_inputs"] or not info["note_inputs"][0]["dialects"] & 1:
    raise ValueError("choose an instrument with a native note-expression port")
port = info["note_inputs"][0]["id"]
reference = {"format": info["format"], "path": info["path"], "id": info["id"],
             "inputs": [], "outputs": []}
bus = 4
for item in info["inputs"]:
    reference["inputs"].append({"id": item["id"], "channels": item["channels"], "bus": bus})
    bus += item["channels"]
bus = 0
for item in info["outputs"]:
    reference["outputs"].append({"id": item["id"], "channels": item["channels"], "bus": bus})
    bus += item["channels"]
# A tmp_ name keeps the def out of the data directory: a stored plugin def
# is probed again on every boot, and this one means nothing to a later session.
PluginDef("tmp_persistent_keys", reference).send(server)

# %%
# The instrument is outside the governed group, so live input keeps processing
# during a stop. Only the lane's notes are released by the transport gesture.
governed = Group(server=server)
voice = Synth("tmp_persistent_keys", target=0, server=server)
server.set_transport(0, 2.0)
server.transport_group(governed)
server.lane_new(3100, governed)
server.lane_set(3100, {"notes": [
    [12000, 48000, {"plugin": voice.id, "port": port}, {"key": 64, "velocity": 0.15}, "gate"],
    [60000, 108000, {"plugin": voice.id, "port": port}, {"key": 69, "velocity": 0.15}, "gate"],
]})
server.transport_play()

# %%
def phrase():
    voice.plugin_event({"kind": "note_on", "source": 1, "note_id": 1,
                        "port": port, "channel": 0, "key": 60, "value": 0.15})
    voice.plugin_event({"kind": "note_on", "source": 1, "note_id": 2,
                        "port": port, "channel": 0, "key": 67, "value": 0.15})
    yield 0.5
    voice.plugin_event({"kind": "expression", "source": 1, "note_id": 1,
                        "port": port, "channel": 0, "key": 60, "id": 6, "value": 0.4})
    yield 0.5
    voice.plugin_event({"kind": "note_off", "source": 1, "note_id": 1,
                        "port": port, "channel": 0, "key": 60, "value": 0.0})
    yield 1.0
    voice.plugin_event({"kind": "note_off", "source": 1, "note_id": 2,
                        "port": port, "channel": 0, "key": 67, "value": 0.0})


Routine(phrase).play()

# %%
def finish():
    server.transport_stop()
    server.lane_free(3100)
    voice.free()
    governed.free()
    session.close()


if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        session.run(3.0)
        assert server.status().synths == 1, "notes must share one persistent instance"
    finally:
        finish()
