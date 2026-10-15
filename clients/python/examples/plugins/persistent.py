#!/usr/bin/env python3
"""One persistent native instrument, live notes and a server-owned event lane.

The installed Linux package includes the CLAP/VST3 helper; in a checkout run
`scripts/refresh-bin.sh` first. Run it once to list the instruments with a
native note-expression port the server found, then name one by what it is
called:

    python clients/python/examples/plugins/persistent.py
    PLUGIN="Surge XT" python clients/python/examples/plugins/persistent.py

The server looks in the standard CLAP and VST3 directories; set `PLUGIN_PATH`
to a directory or binary elsewhere, and `PLUGIN_FORMAT` (`clap` or `vst3`)
when the name is installed in both. The page lists the same instruments to
choose from. It needs an instrument with a native
note-expression port and an audible initial preset; an effects collection has
none. A sampler also needs samples in its initial state. The numeric
repository fixture can verify events but is not a musical preset. This is a
`# %%` notebook: the same calls and event sequence as the web page. Python can
boot a native process; the page attaches to one started separately.
"""
# %%
import os
import sys
from clausters import Group, PluginDef, Routine, Session, Synth

NAME = os.environ.get("PLUGIN", "")
PATHS = [os.environ["PLUGIN_PATH"]] if os.environ.get("PLUGIN_PATH") else []

session = Session.live(port=57113).activate()
session.clock.set_tempo(2.0)
server = session.server

# %%
# The catalog is what the server found installed; its instruments with a
# native note-expression port are listed, each named by what it is called and
# its format.
catalog = server.plugins(*PATHS, timeout=60.0)
instruments = [plugin for plugin in catalog.instruments()
               if plugin.note_inputs and plugin.note_inputs[0]["dialects"] & 1]
if not NAME:
    for plugin in instruments:
        print(f"{plugin.name} ({plugin.format}, {plugin.vendor})")
    session.close()
    raise ValueError("set PLUGIN to one of the instruments listed above")
# The one named carries its ports, parameters and identity.
keys = catalog.find(NAME, format=os.environ.get("PLUGIN_FORMAT"), kind="instrument")
if not keys.note_inputs or not keys.note_inputs[0]["dialects"] & 1:
    raise ValueError("choose an instrument with a native note-expression port")
# Its main output is bus 0. A tmp_ name keeps the def out of the data
# directory: a stored plugin def is probed again on every boot, and this one
# means nothing to a later session.
PluginDef("tmp_persistent_keys", keys).send(server)

# %%
# The instrument is outside the governed group, so live input keeps processing
# during a stop. Only the lane's notes are released by the transport gesture.
# Notes go to the instrument's main note input.
governed = Group(server=server)
voice = Synth("tmp_persistent_keys", target=0, server=server)
server.set_transport(0, 2.0)
server.transport_group(governed)
server.lane_new(3100, governed)
server.lane_set(3100, {"notes": [
    [12000, 48000, {"node": voice.id}, {"key": 64, "velocity": 0.15}, "gate"],
    [60000, 108000, {"node": voice.id}, {"key": 69, "velocity": 0.15}, "gate"],
]})
server.transport_play()

# %%
def phrase():
    voice.plugin_event({"kind": "note_on", "source": 1, "note_id": 1,
                        "channel": 0, "key": 60, "value": 0.15})
    voice.plugin_event({"kind": "note_on", "source": 1, "note_id": 2,
                        "channel": 0, "key": 67, "value": 0.15})
    yield 0.5
    voice.plugin_event({"kind": "expression", "source": 1, "note_id": 1,
                        "channel": 0, "key": 60, "id": 6, "value": 0.4})
    yield 0.5
    voice.plugin_event({"kind": "note_off", "source": 1, "note_id": 1,
                        "channel": 0, "key": 60, "value": 0.0})
    yield 1.0
    voice.plugin_event({"kind": "note_off", "source": 1, "note_id": 2,
                        "channel": 0, "key": 67, "value": 0.0})


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
