"""Device sweep contracts. A driver writes raw JSON per fresh-server trial."""
import json
import math


def validate_sweep(step):
    levels = step.get("levels")
    if not isinstance(levels, list) or not levels or any(type(n) is not int or n < 0 for n in levels) or levels != sorted(set(levels)):
        raise ValueError("Sweep levels must be increasing nonnegative integers")
    for key, default in (("representative_cpu", 35), ("stop_peak_cpu", 90), ("min_fps", 20)):
        value = step.get(key, default)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
            raise ValueError("Invalid sweep threshold: " + key)
    if step.get("gui") and step.get("screen_on") is not True:
        raise ValueError("GUI sweep requires explicit screen_on acknowledgement")
    if not any("{level}" in arg for arg in step["argv"]) or not any("{capture}" in arg for arg in step["argv"]):
        raise ValueError("Sweep argv must name {level} and {capture}")


def assess_capture(capture, gui=False, min_fps=20):
    samples = capture["samples"]
    if not samples:
        raise ValueError("Empty device capture")
    cpus = [s["status"]["avg_cpu"] for s in samples]
    peaks = [s["status"]["peak_cpu"] for s in samples]
    if any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or v < 0 for v in cpus + peaks):
        raise ValueError("Invalid device CPU counters")
    start, end = capture["start"], capture["end"]
    seconds = end["status"]["t"] - start["status"]["t"]
    if not math.isfinite(seconds) or seconds <= 0:
        raise ValueError("Invalid capture duration")
    timing0, timing1 = start["server_timing"], end["server_timing"]
    if not timing0 or not timing1 or timing1["duration"]["count"] <= timing0["duration"]["count"]:
        raise ValueError("No instrumented audio callbacks observed")
    for point in (start, end):
        counters = [point["status"]["late_blocks"], point["server_timing"]["duration"]["count"], point["server_timing"]["over_deadline"]]
        if gui and point.get("gui_timing"):
            counters.append(point["gui_timing"]["duration"]["count"])
        if any(type(v) is not int or v < 0 for v in counters):
            raise ValueError("Device counters must be nonnegative integers")
    late = end["status"]["late_blocks"] - start["status"]["late_blocks"]
    over = timing1["over_deadline"] - timing0["over_deadline"]
    if late < 0 or over < 0:
        raise ValueError("Device counters reset during capture")
    result = {"mean_cpu": sum(cpus) / len(cpus), "peak_cpu": max(peaks), "late": late, "over_deadline": over}
    if gui:
        first, last = start["gui_timing"], end["gui_timing"]
        frames = last["duration"]["count"] - first["duration"]["count"] if first and last else 0
        result["fps"] = frames / seconds
        if result["fps"] < min_fps:
            raise ValueError(f"GUI did not draw: {result['fps']:.2f} fps; keep screen on")
    return result


def execute_sweep(step, argv, cwd, env, campaign, repetition, execute):
    folder = campaign / "evidence" / f"{step['id']}-{repetition}"
    folder.mkdir()
    receipts, selected = [], None
    overall = {"status": "passed", "exit_code": 0, "elapsed_seconds": 0}
    error = None
    for level in step["levels"]:
        capture = folder / f"{level}.json"
        args = [a.replace("{level}", str(level)).replace("{capture}", str(capture)) for a in argv]
        try:
            outcome = execute(args, cwd, env, step["timeout_seconds"], folder / f"{level}.stdout", folder / f"{level}.stderr")
        except (OSError, ValueError) as exc:
            outcome = {"status": "error", "exit_code": None, "elapsed_seconds": 0}
            error = str(exc)
        overall["elapsed_seconds"] += outcome["elapsed_seconds"]
        receipt = {"argv": args, "cwd": str(cwd), "environment": __import__("controls").receipt_environment(env), "level": level, **outcome}
        receipts.append(receipt)
        if outcome["status"] != "passed":
            overall.update(status=outcome["status"], exit_code=outcome["exit_code"])
            break
        try:
            data = json.loads(capture.read_text())
            assessed = assess_capture(data["capture"], step.get("gui", False), step.get("min_fps", 20))
            receipt["assessment"] = assessed
            stop = assessed["peak_cpu"] >= step.get("stop_peak_cpu", 90) or assessed["late"] > 0 or assessed["over_deadline"] > 0
            if not stop and assessed["mean_cpu"] <= step.get("representative_cpu", 35):
                selected = level
            if stop:
                receipt["stopped_at_safety_limit"] = True
                break
        except (ValueError, KeyError, OSError, TypeError) as exc:
            overall.update(status="error", exit_code=None)
            error = str(exc)
            break
    if selected is None and overall["status"] == "passed":
        overall.update(status="error", exit_code=None)
        error = "No safe tested level satisfies representative CPU limit"
    from audit import write
    write(folder / "receipt.json", {"commands": receipts, "representative_level": selected, "error": error})
    # Uniform top-level logs exist even for a driver failure.
    (campaign / "logs" / f"{step['id']}-{repetition}.stdout").write_text("")
    (campaign / "logs" / f"{step['id']}-{repetition}.stderr").write_text(error or "")
    return overall
