"""Regenerates `vectors.json`: SuperCollider's own output for every case in
`cases.py`, which `tests/offline/sc_reference.rs` holds the server's UGens
against.

    python3 crates/clausters-server/tests/sc_reference/gen.py ~/src/supercollider

It compiles the base library's plugins from that checkout into shared objects
(the groups scsynth's own build makes), compiles `harness.cpp` against the
same headers, runs every case through it and writes the cases together with
what each probe read. Nothing of SuperCollider is kept in the repository but
those numbers, and the build lives in `target/sc-reference/`, so a checkout
is needed only to regenerate them -- the comparison itself runs without one.

The SuperCollider revision the vectors came from is written into the file,
so a change in them can be read against a change upstream.
"""

import json
import math
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
BUILD = ROOT / "target" / "sc-reference"

sys.path.insert(0, str(HERE))
import cases as case_table  # noqa: E402

# The plugin libraries scsynth's build makes, by the sources each holds
# (`server/plugins/CMakeLists.txt`). The platform ones (Bela, the mouse and
# keyboard, Ableton Link) and the disk streamers are left out.
LIBRARIES = {
    name: [name + ".cpp"]
    for name in [
        "BinaryOpUGens",
        "ChaosUGens",
        "DelayUGens",
        "DemandUGens",
        "DynNoiseUGens",
        "FilterUGens",
        "GendynUGens",
        "GrainUGens",
        "IOUGens",
        "LFUGens",
        "MulAddUGens",
        "NoiseUGens",
        "OscUGens",
        "PanUGens",
        "PhysicalModelingUGens",
        "ReverbUGens",
        "TestUGens",
        "TriggerUGens",
        "UnaryOpUGens",
        "UnpackFFTUGens",
    ]
}
LIBRARIES["FFT_UGens"] = [
    "FFTInterfaceTable.cpp",
    "FFT_UGens.cpp",
    "PV_UGens.cpp",
    "PartitionedConvolution.cpp",
]
LIBRARIES["PV_ThirdParty"] = [
    "Convolution.cpp",
    "FFT2InterfaceTable.cpp",
    "FeatureDetection.cpp",
    "PV_ThirdParty.cpp",
]
LIBRARIES["ML_UGens"] = [
    "ML.cpp",
    "Loudness.cpp",
    "BeatTrack.cpp",
    "Onsets.cpp",
    "onsetsds.c",
    "KeyTrack.cpp",
    "MFCC.cpp",
    "BeatTrack2.cpp",
    "ML_SpecStats.cpp",
]


def flags(sc: Path) -> list[str]:
    return [
        "-O2",
        "-fPIC",
        "-w",
        "-DSC_LINUX",
        "-DNOVA_SIMD",
        "-DSC_FFT_GREEN",
        "-DNDEBUG",
        f"-I{sc}/include/plugin_interface",
        f"-I{sc}/include/common",
        f"-I{sc}/include/server",
        f"-I{sc}/common",
        f"-I{sc}/server/plugins",
        f"-I{sc}/external_libraries/nova-simd",
        f"-I{sc}/external_libraries/nova-tt",
        f"-I{sc}/external_libraries/boost",
        f"-I{sc}/external_libraries/libsndfile",
    ]


def compiler(source: str) -> list[str]:
    return ["gcc"] if source.endswith(".c") else ["g++", "-std=c++17"]


def build_library(sc: Path, name: str, sources: list[str]) -> None:
    out = BUILD / "plugins" / f"{name}.so"
    srcs = [sc / "server" / "plugins" / s for s in sources]
    if out.exists() and all(out.stat().st_mtime > s.stat().st_mtime for s in srcs):
        return
    objs = []
    for s in srcs:
        obj = BUILD / "obj" / (s.name + ".o")
        subprocess.run([*compiler(s.name), *flags(sc), "-c", str(s), "-o", str(obj)], check=True)
        objs.append(str(obj))
    subprocess.run(["g++", "-shared", "-o", str(out), *objs], check=True)


def build(sc: Path) -> Path:
    (BUILD / "plugins").mkdir(parents=True, exist_ok=True)
    (BUILD / "obj").mkdir(parents=True, exist_ok=True)
    with ThreadPoolExecutor(max_workers=8) as pool:
        for f in [pool.submit(build_library, sc, n, s) for n, s in LIBRARIES.items()]:
            f.result()
    harness = BUILD / "harness"
    sources = [
        HERE / "harness.cpp",
        sc / "common" / "Samp.cpp",
        sc / "common" / "SC_fftlib.cpp",
    ]
    subprocess.run(
        [
            "g++",
            "-std=c++17",
            *flags(sc),
            *map(str, sources),
            "-x",
            "c",
            str(sc / "common" / "fftlib.c"),
            "-x",
            "none",
            "-o",
            str(harness),
            "-ldl",
        ],
        check=True,
    )
    return harness


def protocol(case: dict) -> str:
    """One case as the harness's input lines."""
    lines = [f"case {case['name']} {case['blocks']} {case.get('seed', 0)}"]
    for b in case.get("buffers", []):
        values = " ".join(repr(float(v)) for v in b["data"])
        lines.append(f"buf {b['num']} {b['channels']} {b['frames']} {b.get('sr', 48000.0)} {values}")
    ugens = case["ugens"]
    for u in ugens:
        sc = u.get("sc", {})
        kind = sc.get("kind", u["kind"])
        special = sc.get("special", 0)
        if "op" in u and "special" not in sc:
            special = case_table.SPECIAL[u["op"]]
        ins = sc.get("inputs", u.get("inputs", []))
        toks = []
        for i in ins:
            if "const" in i:
                toks.append(f"c:{float(i['const'])!r}")
            else:
                src = ugens[i["ugen"]]
                out = i.get("output", src.get("sc", {}).get("output", 0))
                toks.append(f"u:{i['ugen']}:{out}")
        outputs = sc.get("outputs", 1)
        lines.append(f"ugen {kind} {u.get('rate', 'ar')} {special} {outputs} {len(toks)} {' '.join(toks)}")
    probe = case.get("probe", len(ugens) - 1)
    out = ugens[probe].get("sc", {}).get("output", 0)
    lines.append(f"probe {probe} {out}")
    lines.append("run")
    return "\n".join(lines) + "\n"


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sc = Path(sys.argv[1]).expanduser().resolve()
    harness = build(sc)
    cases = case_table.cases()
    names = [c["name"] for c in cases]
    if len(set(names)) != len(names):
        sys.exit("gen.py: two cases share a name")
    feed = "".join(protocol(c) for c in cases)
    out = subprocess.run(
        [str(harness), str(BUILD / "plugins")], input=feed, capture_output=True, text=True, check=False
    )
    if out.returncode != 0:
        sys.exit(f"harness failed:\n{out.stderr}")
    got: dict[str, dict] = {}
    for line in out.stdout.splitlines():
        if not line.startswith("= "):
            continue
        name, *rest = line[2:].split(" ")
        entry = got.setdefault(name, {})
        if rest[:1] == ["done"]:
            entry["done"] = int(rest[1])
        else:
            entry["values"] = [float(v) for v in rest]
    for c in cases:
        if "values" not in got.get(c["name"], {}):
            sys.exit(f"gen.py: no output for {c['name']}")
        # JSON has no NaN or infinity, so those travel as their names.
        c["expect"] = [v if math.isfinite(v) else str(v) for v in got[c["name"]]["values"]]
        if "done" in got[c["name"]]:
            c["done"] = got[c["name"]]["done"]
    rev = subprocess.run(
        ["git", "-C", str(sc), "describe", "--always", "--dirty"], capture_output=True, text=True
    ).stdout.strip()
    doc = {"supercollider": rev, "cases": cases}
    text = json.dumps(doc, indent=None, separators=(",", ":"), allow_nan=False)
    # One case per line keeps a diff readable.
    text = text.replace('{"name"', '\n{"name"')
    (HERE / "vectors.json").write_text(text + "\n")
    print(f"{len(cases)} cases from SuperCollider {rev}", file=sys.stderr)
    if os.environ.get("SC_REFERENCE_STDERR"):
        print(out.stderr, file=sys.stderr)


if __name__ == "__main__":
    main()
