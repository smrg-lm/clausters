#!/usr/bin/env python3
"""A native effect's editor on the server's machine, with a continuous quiet tone.

The installed Linux package includes the CLAP/VST3 helper; in a checkout run
`scripts/refresh-bin.sh` first. Name the effect by what it is called, as
`server.plugins()` lists them, and run:

    PLUGIN="Parametric Equalizer x8 Stereo" \
        python clients/python/examples/plugins/editor.py

The server looks in the standard CLAP and VST3 directories; set `PLUGIN_PATH`
to a directory or binary elsewhere, and `PLUGIN_FORMAT` (`clap` or `vst3`)
when the name is installed in both. Choose a mono/stereo effect with an
audible default preset and an X11 editor; the parametric equalizer above is
one. The repository CLAP gain and VST3 fixtures also work, named `gain` and
`Clausters VST3 fixture` with `PLUGIN_PATH` at the built binary: their editors
exercise callbacks and change gain to 0.75. Close/reopen the editor and edit
its controls while the tones keep playing. Run as a script, Enter finishes;
cell by cell, `finish()` closes the session. Python boots the native server;
the paired page attaches to one booted separately.
"""
# %%
import os
import sys
from clausters import PluginDef, Session, Synth, SynthDef
from clausters.defs import out, sine

NAME = os.environ.get("PLUGIN", "")
if not NAME:
    raise ValueError("set PLUGIN to the name of a native effect")
PATHS = [os.environ["PLUGIN_PATH"]] if os.environ.get("PLUGIN_PATH") else []
session = Session.live(port=57113).activate()
server = session.server

# %%
# The catalog is what the server found installed: the effect is found by its
# name, and carries its ports, parameters and identity.
effect = server.plugins(*PATHS, timeout=60.0).find(
    NAME, format=os.environ.get("PLUGIN_FORMAT"), kind="effect")
if not effect.has_editor:
    raise ValueError("choose an effect with a native X11 editor")
# Its main input reads bus 4, where the tones are; its main output is bus 0.
# A tmp_ name keeps the def out of the data directory: a stored plugin def
# is probed again on every boot, and this one means nothing to a later session.
PluginDef("tmp_editor_effect", effect, inputs=4).send(server)
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
