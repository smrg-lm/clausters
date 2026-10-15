#!/usr/bin/env python3
"""A native effect editor on the server's machine, with a continuous quiet tone.

The installed Linux package includes the CLAP/VST3 helper; in a checkout run
`scripts/refresh-bin.sh` first. Name the effect with two variables -- its binary
and its native ID, as `server.scan_plugins(binary)` lists them -- and run:

    PLUGIN_BINARY=/usr/lib/clap/lsp-plugins.clap \
    PLUGIN_ID=in.lsp-plug.para_equalizer_x8_stereo \
        python clients/python/examples/plugins/editor.py

Choose a mono/stereo effect with an audible default preset and an X11 editor;
the parametric equalizer above is one. The repository CLAP gain and VST3
fixtures also work: their editors exercise callbacks and change gain to 0.75.
Close/reopen the editor and edit its controls while the tones keep playing.
Run as a script, Enter finishes; cell by cell, `finish()` closes the session.
Python boots the native server; the paired page attaches to one booted
separately.
"""
# %%
import os
import sys
from clausters import PluginDef, Session, Synth, SynthDef
from clausters.defs import out, sine

BINARY = os.environ.get("PLUGIN_BINARY", "")
PLUGIN_ID = os.environ.get("PLUGIN_ID", "")
if not BINARY or not PLUGIN_ID:
    raise ValueError("set PLUGIN_BINARY and PLUGIN_ID to a native effect")
session = Session.live(port=57113).activate()
server = session.server

# %%
rows = server.scan_plugins(BINARY, timeout=60.0)
info = next(row["plugin"] for row in rows
            if row["kind"] == "plugin" and row["plugin"]["id"] == PLUGIN_ID)
if not info["editor"] or not info["inputs"]:
    raise ValueError("choose an effect with a native X11 editor")
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
PluginDef("tmp_editor_effect", reference).send(server)
SynthDef("editor_tone", out(4.0, sine(220.0) * 0.08),
         out(5.0, sine(330.0) * 0.08)).send(server)
source = Synth("editor_tone", server=server)
voice = Synth("tmp_editor_effect", server=server)
server.sync()

# %%
voice.open_plugin_editor()
print(voice.query_plugin_editor())
voice.close_plugin_editor()
voice.open_plugin_editor()

# %%
def finish():
    voice.close_plugin_editor()
    voice.free()
    source.free()
    session.close()


if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        input("Edit the native plugin while the tones play; press Enter to finish. ")
    finally:
        finish()
else:
    print("Editor open; finish() closes the session.")
