#!/usr/bin/env python3
"""Sample Linux process CPU/RSS externally; never interpret this as frame timing."""
import argparse
import json
import math
import os
from pathlib import Path
import time


def decode_stat(text, ticks, page_size):
    fields = text.rsplit(") ", 1)[1].split()
    return {"cpu_seconds": (int(fields[11]) + int(fields[12])) / ticks,
            "start_ticks": int(fields[19]), "rss_bytes": int(fields[21]) * page_size}


def sample(pid, seconds, interval, output):
    if not math.isfinite(seconds) or not math.isfinite(interval) or pid <= 0 or seconds <= 0 or interval <= 0 or interval > seconds / 2:
        raise ValueError("Need a positive PID/duration and at least two sample intervals")
    output = Path(output)
    ticks, page_size = os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE")
    end = time.monotonic() + seconds
    previous, identity = None, None
    with output.open("x") as stream:
        while True:
            current = decode_stat(Path(f"/proc/{pid}/stat").read_text(), ticks, page_size)
            stamp = time.monotonic()
            if identity is not None and current["start_ticks"] != identity:
                raise ValueError("PID identity changed during capture")
            identity = current["start_ticks"]
            row = {"pid": pid, "monotonic_seconds": stamp, **current}
            if previous:
                row["cpu_percent_one_core"] = 100 * (current["cpu_seconds"] - previous["cpu_seconds"]) / (stamp - previous["monotonic_seconds"])
            stream.write(json.dumps(row) + "\n")
            stream.flush()
            previous = row
            if stamp >= end:
                break
            time.sleep(min(interval, end - stamp))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--interval", type=float, default=.1)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    sample(args.pid, args.seconds, args.interval, args.output)
