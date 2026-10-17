"""Optional Linux GPU counters; never infer presentation from engine activity."""
from pathlib import Path
import json
import subprocess
import time


def parse_fdinfo(text):
    fields = dict(line.split(":", 1) for line in text.splitlines() if ":" in line)
    fields = {key: value.strip() for key, value in fields.items()}
    if not fields.get("drm-driver") or not fields.get("drm-client-id"):
        return None
    engines = {}
    for key, value in fields.items():
        if key.startswith("drm-engine-") and not key.startswith("drm-engine-capacity-"):
            parts = value.split()
            if len(parts) != 2 or parts[1] != "ns" or not parts[0].isdigit():
                raise ValueError("Unsupported DRM engine counter: " + key)
            engines[key.removeprefix("drm-engine-")] = int(parts[0])
    return {"driver": fields["drm-driver"], "device": fields.get("drm-pdev"),
            "client": fields["drm-client-id"], "engines_ns": engines}


def gpu_sample(pids, proc=Path("/proc")):
    clients, raw, errors = {}, {}, []
    for pid in pids:
        try:
            entries = sorted((proc / str(pid) / "fdinfo").iterdir())
        except OSError as exc:
            errors.append(str(exc))
            continue
        for entry in entries:
            try:
                text = entry.read_text()
                client = parse_fdinfo(text)
                if client is None:
                    continue
                raw[f"{pid}/{entry.name}"] = text
                # Duplicated/shared descriptors must not count twice. A driver
                # without a PCI identity is scoped to its DRM device number.
                device = client["device"]
                if device is None:
                    device = str((proc / str(pid) / "fd" / entry.name).stat().st_rdev)
                key = f"{client['driver']}:{device}:{client['client']}"
                if key in clients:
                    for engine, value in client["engines_ns"].items():
                        counters = clients[key]["engines_ns"]
                        counters[engine] = max(counters.get(engine, 0), value)
                else:
                    clients[key] = client
            except (OSError, ValueError) as exc:
                errors.append(f"{entry}: {exc}")
    available = any(c["engines_ns"] for c in clients.values()) and not errors
    return {"t": time.monotonic(), "status": "available" if available else "unsupported",
            "reason": None if available else "No readable per-client DRM engine time counters",
            "clients": clients, "raw_fdinfo": raw, "errors": errors,
            "presentation": {"status": "unsupported", "reason": "DRM fdinfo does not measure presentation latency"}}


def gpu_delta(samples):
    if len(samples) < 2 or any(s["status"] != "available" for s in samples):
        return {"status": "unsupported", "reason": "GPU counters unavailable for the complete interval"}
    first, last = samples[0], samples[-1]
    if last["t"] <= first["t"]:
        raise ValueError("Invalid GPU sample interval")
    identity = lambda s: {(key, engine) for key, client in s["clients"].items() for engine in client["engines_ns"]}
    keys = identity(first)
    if any(identity(s) != keys for s in samples):
        return {"status": "unsupported", "reason": "DRM client/engine coverage changed during capture"}
    # The kernel permits temporary counter decreases. Keep a high-water mark
    # and wait for catch-up rather than treating a negative delta as idle.
    high = {(key, engine): first["clients"][key]["engines_ns"][engine] for key, engine in keys}
    for sample in samples[1:]:
        for key, engine in keys:
            high[key, engine] = max(high[key, engine], sample["clients"][key]["engines_ns"][engine])
    if any(last["clients"][key]["engines_ns"][engine] < high[key, engine] for key, engine in keys):
        return {"status": "unsupported", "reason": "DRM counters did not catch up before the interval ended"}
    seconds = last["t"] - first["t"]
    rows = [{"client": key, "engine": engine,
             "busy_ns": high[key, engine] - first["clients"][key]["engines_ns"][engine]}
            for key, engine in sorted(keys)]
    for row in rows:
        row["busy_percent"] = row["busy_ns"] / (seconds * 1e9) * 100
    return {"status": "available", "elapsed_seconds": seconds, "engines": rows,
            "scope": "Per-client GPU engine busy time, not per-frame GPU duration or presentation latency"}


def host_sample():
    def read(path):
        try:
            return Path(path).read_text().strip()
        except OSError:
            return None
    temperatures = {}
    for zone in Path("/sys/class/thermal").glob("thermal_zone*"):
        temperatures[zone.name] = {"type": read(zone / "type"), "millidegrees_c": read(zone / "temp")}
    return {"loadavg": read("/proc/loadavg"), "temperatures": temperatures}


def pipewire_nodes(pids):
    try:
        result = subprocess.run(["pw-dump", "-N"], capture_output=True, text=True, timeout=10)
        objects = json.loads(result.stdout) if result.returncode == 0 else []
        nodes = {item["id"] for item in objects if item.get("type") == "PipeWire:Interface:Node"
                 and str(item.get("info", {}).get("props", {}).get("application.process.id")) in set(map(str, pids))}
        # Include the connected output sink, whose xruns can differ from the
        # stream's. Do not fold unrelated audio clients into a process counter.
        peers = set()
        for item in objects:
            info = item.get("info", {})
            if item.get("type") == "PipeWire:Interface:Link" and info.get("output-node-id") in nodes:
                peers.add(info["input-node-id"])
        return {"status": "available" if nodes else "unsupported", "stream_nodes": sorted(nodes),
                "sink_nodes": sorted(peers), "stdout": result.stdout, "stderr": result.stderr,
                "exit_code": result.returncode}
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        return {"status": "unsupported", "reason": str(exc)}


def xrun_delta(identity, before, after):
    if identity.get("status") != "available" or any(s.get("status") != "available" for s in (before, after)):
        return {"status": "unsupported", "reason": "PipeWire node identities or counters unavailable"}
    ids = identity["stream_nodes"] + identity["sink_nodes"]
    first, last = ({row["id"]: row for row in snapshot["rows"]} for snapshot in (before, after))
    if any(node not in first or node not in last or type(first[node]["err"]) is not int
           or type(last[node]["err"]) is not int or last[node]["err"] < first[node]["err"] for node in ids):
        return {"status": "unsupported", "reason": "PipeWire counters missing or reset"}
    return {"status": "available", "nodes": {str(node): last[node]["err"] - first[node]["err"] for node in ids}}
