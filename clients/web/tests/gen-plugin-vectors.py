#!/usr/bin/env python3
"""Generate plugin-vectors.json from the Python client's plugin catalog.

The Python client is the reference: this freezes what its `PluginCatalog`
reads from a set of scan rows (one CLAP and one VST3 plugin of each kind, as
the server's discovery reports them, and a failed binary), what its searches
find and what references its `PluginDef` builds, so
`tests/plugin-parity.test.ts` can assert the web client's catalog, searches
and defs are the same.

The JSON is committed; regenerate with:

    python3 gen-plugin-vectors.py

(from clients/web/tests/, with the Python client importable and its native
core built).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters import PluginCatalog, PluginDef  # noqa: E402


def port(i, name, channels, main, layout):
    return {"id": i, "name": name, "channels": channels, "flags": int(main),
            "main": main, "layout": layout, "active": True}


def parameter(i, name, module=""):
    return {"id": i, "name": name, "module": module, "flags": 0,
            "min": 0.0, "max": 1.0, "step": 0.0, "default": 0.5}


def plugin(fmt, name, features, inputs, notes, parameters):
    return {"kind": "plugin", "plugin": {
        "format": fmt, "path": f"/plugins/{name}.{fmt}", "id": f"org.test.{name.lower()}",
        "name": name, "vendor": "Test", "version": "1", "description": "", "url": "",
        "manual_url": "", "support_url": "", "features": features, "sdk_version": "",
        "inputs": inputs, "outputs": [port(20, "Main Out", 2, True, "stereo")],
        "note_inputs": [{"id": 0, "name": "Notes", "dialects": 3, "preferred": 1,
                         "channels": 16}] if notes else [],
        "note_outputs": [], "parameters": parameters, "latency": 0, "state": True,
        "editor": True}}


ROWS = [
    plugin("clap", "Keys", ["instrument", "piano", "stereo"], [], True,
           [parameter(1, "Volume"), parameter(2, "Hall Mix", "Room")]),
    plugin("vst3", "Keys", ["Instrument", "Piano"], [], True,
           [parameter(7, "Volume")]),
    plugin("clap", "Plate", ["audio-effect", "reverb"],
           [port(10, "Main In", 2, True, "stereo"), port(11, "Side Chain", 1, False, "mono")],
           False, [parameter(3, "Mix (%)"), parameter(4, "mix"), parameter(5, "Decay")]),
    plugin("vst3", "Scope", ["Fx", "Analyzer"], [port(0, "In", 2, True, "stereo")], False, []),
    {"kind": "error", "path": "/plugins/broken.clap", "error": "plugin helper timeout"},
]

FINDS = [
    ["keys", {}],
    ["keys", {"format": "vst3"}],
    [None, {"tag": "reverb"}],
    [None, {"kind": "analyzer"}],
    ["organ", {}],
]

FILTERS = [
    [None, {}],
    [None, {"kind": "instrument"}],
    ["e", {"vendor": "test"}],
    [None, {"format": "CLAP"}],
]

REFERENCES = [
    ["Plate", {}],
    ["Plate", {"inputs": 4}],
    ["Plate", {"inputs": {"in_side_chain": 8}, "outputs": 2, "state": [1, 2]}],
    ["Plate", {"inputs": {"in_aux": 8}}],
    ["Scope", {"inputs": {"in": 6}, "outputs": None}],
]


def answer(call):
    try:
        return {"value": call()}
    except ValueError as error:
        return {"error": str(error)}


def main():
    catalog = PluginCatalog.from_rows(ROWS)
    vectors = {
        "rows": ROWS,
        "catalog": {"plugins": [p.data() for p in catalog], "errors": catalog.errors},
        "finds": [{"name": name, "query": query,
                   "result": answer(lambda: catalog.plugins.index(catalog.find(name, **query)))}
                  for name, query in FINDS],
        "filters": [{"name": name, "query": query,
                     "result": [catalog.plugins.index(p) for p in catalog.filter(name, **query)]}
                    for name, query in FILTERS],
        "references": [{"plugin": name, "options": options,
                        "result": answer(lambda: PluginDef(
                            "def", catalog.find(name, format="clap" if name == "Plate" else None),
                            inputs=options.get("inputs"), outputs=options.get("outputs", 0),
                            state=bytes(options["state"]) if "state" in options else None,
                        ).reference)}
                       for name, options in REFERENCES],
    }
    out = pathlib.Path(__file__).with_name("plugin-vectors.json")
    out.write_text(json.dumps(vectors, indent=1) + "\n")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
