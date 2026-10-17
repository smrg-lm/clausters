#!/usr/bin/env python3
"""Collect alternating device controls. Emits harness metric JSONL on success."""
import argparse
import json
import math
import os
from pathlib import Path
import shutil
import sys

from audit import digest, execute, write
from controls import receipt_environment


CONDITIONS = [(mode, instrumented) for mode in ("audio", "gui", "combined")
              for instrumented in (True, False)]
# The geometry of instrumentation/timing.rs.in, including the overflow bucket.
BUCKET_COUNT = 8192


def order(repetition):
    # Reverse successive passes; the third rotates to avoid a fixed first run.
    if repetition % 3 == 0:
        return CONDITIONS
    if repetition % 3 == 1:
        return list(reversed(CONDITIONS))
    return CONDITIONS[2:] + CONDITIONS[:2]


def compatibility(root):
    root = Path(root)
    receipt = json.loads((root / "receipt.json").read_text())
    profiles = {}
    for trial in receipt["trials"]:
        if trial["exit_code"] != 0 or digest(root / trial["capture"]) != trial["sha256"]:
            raise ValueError("Paired trial failed or capture identity changed")
        record = json.loads((root / trial["capture"]).read_text())
        timing = record["capture"]["end"].get("server_timing")
        profile = {"nodes": record["args"]["nodes"], "seconds": record["args"]["seconds"],
                   "warmup": record["args"]["warmup"], "quantum_request": record["env"]["PIPEWIRE_QUANTUM"],
                   "actual_sample_rate": record["capture"]["end"]["status"]["actual_sample_rate"],
                   "callback_frames": [timing["frames_min"], timing["frames_max"]] if timing else None,
                   "renderer": record.get("renderer"),
                   "gpu_scope": record["gpu_assessment"]["status"],
                   "xrun_scope": record.get("xrun_assessment", {}).get("status", "unsupported")}
        previous = profiles.setdefault(trial["condition"], profile)
        if previous != profile:
            raise ValueError("Device configuration changed between paired repetitions")
    return {"conditions": profiles, "binaries": {key: value["sha256"] for key, value in receipt["binaries"].items()}}


def histogram_delta(before, after):
    width = after["bucket_ns"]
    if type(width) is not int or width <= 0 or width != before["bucket_ns"]:
        raise ValueError("Histogram geometry changed")
    def decode(histogram):
        # The producer emits only occupied buckets as [index, count] pairs.
        # New occupied indices between snapshots do not change its geometry.
        decoded = [0] * BUCKET_COUNT
        seen = set()
        for pair in histogram["buckets"]:
            if not isinstance(pair, list) or len(pair) != 2:
                raise ValueError("Expected sparse histogram index/count pairs")
            index, value = pair
            if (type(index) is not int or not 0 <= index < BUCKET_COUNT
                    or index in seen or type(value) is not int or value < 0):
                raise ValueError("Invalid sparse histogram bucket")
            seen.add(index)
            decoded[index] = value
        return decoded
    buckets = [b - a for a, b in zip(decode(before), decode(after))]
    count = after["count"] - before["count"]
    total = after["sum_ns"] - before["sum_ns"]
    # Writers read cumulative atomics independently. Retain raw endpoints;
    # quantiles use bucket deltas, never the lifetime maximum after warmup.
    if any(v < 0 for v in buckets) or count <= 0 or total < 0 or not sum(buckets):
        raise ValueError("Invalid or empty histogram delta")
    rank, cumulative = math.ceil(sum(buckets) * .99), 0
    p99 = None
    for index, n in enumerate(buckets):
        cumulative += n
        if cumulative >= rank:
            if index == BUCKET_COUNT - 1:
                raise ValueError("p99 is in the overflowing bucket")
            p99 = (index + 1) * width / 1000
            break
    return {"mean_us": total / count / 1000, "p99_upper_us": p99}


def measurements(record, name):
    result = []
    def add(metric, unit, value, direction="lower"):
        if isinstance(value, bool) or not math.isfinite(value) or value < 0:
            raise ValueError("Invalid device metric: " + metric)
        result.append({"name": name + "." + metric, "unit": unit,
                       "value": value, "direction": direction})
    capture = record["capture"]
    add("mean_cpu", "percent", record["assessment"]["mean_cpu"],
        "descriptive" if name.startswith("gui-") else "lower")
    add("late_blocks", "count", record["assessment"]["late"])
    for kind in ("server_timing", "gui_timing"):
        first, last = capture["start"].get(kind), capture["end"].get(kind)
        if first and last:
            for key, value in histogram_delta(first["duration"], last["duration"]).items():
                add(kind + "." + key, "microseconds", value,
                    "lower" if kind == "server_timing" and not name.startswith("gui-") else "descriptive")
    if "over_deadline" in record["assessment"]:
        add("over_deadline", "count", record["assessment"]["over_deadline"])
    if "fps" in record["assessment"]:
        add("fps", "Hz", record["assessment"]["fps"], "descriptive")
    xruns = record.get("xrun_assessment", {})
    if xruns.get("status") == "available":
        add("pipewire_xruns", "count", sum(xruns["nodes"].values()))
    gpu = record["gpu_assessment"]
    if gpu["status"] == "available":
        # Aggregate by driver/device/engine, not ephemeral DRM client IDs.
        engines = {}
        for row in gpu["engines"]:
            engine = row["client"].rsplit(":", 1)[0] + "." + row["engine"]
            engines[engine] = engines.get(engine, 0) + row["busy_percent"]
        for engine, value in sorted(engines.items()):
            add("gpu." + engine, "percent", value, "descriptive")
    for role in ("server_pids", "gui_pids"):
        pids = record.get(role, [])
        if not pids:
            continue
        cpu = 0
        for pid in pids:
            first = capture["start"]["proc"][str(pid)]
            last = capture["end"]["proc"][str(pid)]
            if not first or not last or last["t"] <= first["t"]:
                raise ValueError("Missing process capture")
            cpu += 100 * (last["cpu_s"] - first["cpu_s"]) / (last["t"] - first["t"])
        add(role + ".cpu", "percent", cpu, "descriptive")
    return result


def collect(args):
    root = Path(args.output).resolve()
    if args.nodes is None:
        receipts = sorted(Path(args.sweep_evidence).resolve().glob("device-sweep-*/receipt.json"))
        selections = [json.loads(path.read_text()) for path in receipts]
        if any(r.get("error") or not r.get("commands")
               or any(c.get("status") != "passed" for c in r["commands"]) for r in selections):
            raise ValueError("Sweep collection failed; do not select stale safe levels")
        levels = [r["representative_level"] for r in selections]
        if len(levels) < 3 or any(type(level) is not int or level < 0 for level in levels):
            raise ValueError("Need three completed safe sweep selections")
        args.nodes = min(levels)
    root.mkdir(parents=True, exist_ok=False)
    binaries = {key: {"path": str(Path(getattr(args, key)).resolve(strict=True)),
                      "sha256": digest(getattr(args, key))}
                for key in ("server", "gui_bin", "control_server", "control_gui", "ffi_lib")}
    receipt = {"configuration": vars(args), "binaries": binaries, "trials": [],
               "limitations": ["GUI-only includes the minimal live-source audio server",
                               "Uninstrumented controls have no callback/frame histograms",
                               "GPU engine time is not presentation latency"],
               "timeline": "source frequency: 880 * 2**(0.5*sin(2*pi*t/11)); stereo weights: cos/sin(pi*(0.5-0.5*cos(2*pi*t/6))); update every 30 ms"}
    rows = []
    write(root / "receipt.json", receipt)
    for repetition in range(args.repetitions):
        for mode, instrumented in order(repetition):
            if shutil.disk_usage(root).free < float(os.environ.get("AUDIT_DISK_FLOOR_GIB", 0)) * 1024 ** 3:
                raise ValueError("Free disk space is below campaign floor")
            name = mode + ("-instrumented" if instrumented else "-control")
            capture = root / f"{repetition}-{name}.json"
            argv = [sys.executable, str(Path(__file__).with_name("device_driver.py")),
                    "--server", binaries["server" if instrumented else "control_server"]["path"],
                    "--gui-bin", binaries["gui_bin" if instrumented else "control_gui"]["path"],
                    "--ffi-lib", binaries["ffi_lib"]["path"],
                    "--nodes", str(0 if mode == "gui" else args.nodes),
                    "--source", "--screen-on", "--seconds", str(args.seconds),
                    "--warmup", str(args.warmup), "--capture", str(capture)]
            if mode != "audio":
                argv.append("--gui")
            if not instrumented:
                argv.append("--uninstrumented")
            outcome = execute(argv, Path.cwd(), os.environ.copy(), args.seconds + args.warmup + 90,
                              capture.with_suffix(".stdout"), capture.with_suffix(".stderr"))
            trial = {"condition": name, "repetition": repetition, "argv": argv,
                     "capture": capture.name, "environment": receipt_environment(os.environ), **outcome}
            receipt["trials"].append(trial)
            write(root / "receipt.json", receipt)
            if outcome["status"] != "passed":
                raise ValueError("Device trial failed: " + str(capture))
            record = json.loads(capture.read_text())
            if record["assessment"]["late"] or record["assessment"].get("over_deadline", 0):
                raise ValueError("Device trial missed a deadline: " + str(capture))
            if any(record.get("xrun_assessment", {}).get("nodes", {}).values()):
                raise ValueError("PipeWire trial observed xruns: " + str(capture))
            trial["sha256"] = digest(capture)
            trial["metrics"] = measurements(record, name)
            rows.extend(trial["metrics"])
            write(root / "receipt.json", receipt)
    for row in rows:
        print(json.dumps(row, allow_nan=False))


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    for key in ("server", "gui-bin", "control-server", "control-gui", "ffi-lib", "output"):
        ap.add_argument("--" + key, required=True)
    load = ap.add_mutually_exclusive_group(required=True)
    load.add_argument("--nodes", type=int)
    load.add_argument("--sweep-evidence")
    ap.add_argument("--repetitions", type=int, default=3)
    ap.add_argument("--seconds", type=float, default=60)
    ap.add_argument("--warmup", type=float, default=10)
    args = ap.parse_args()
    if (args.nodes is not None and args.nodes < 0) or args.repetitions < 3 or not all(math.isfinite(v) and v > 0 for v in (args.seconds, args.warmup)):
        ap.error("Need nonnegative nodes, >=3 repetitions and positive finite durations")
    try:
        collect(args)
    except (OSError, ValueError, KeyError) as exc:
        print(str(exc), file=sys.stderr)
        sys.exit(1)
