#!/usr/bin/env python3
"""Derive a scoped tolerance from reviewed, sealed same-code records."""
import argparse
import itertools
import json
import math
from pathlib import Path
import tarfile

from audit import compare, digest, read, verify_record, write


def archive_identity(record, archive):
    archive = Path(archive).resolve(strict=True)
    sidecar = read(str(archive) + ".json")
    sha = digest(archive)
    if sidecar.get("sha256") != sha or sidecar.get("retrieval_verified") is not True:
        raise ValueError("Archive lacks a matching verified retrieval receipt")
    with tarfile.open(archive) as bundle:
        stream = bundle.extractfile("record/hashes.json")
        if stream is None or stream.read() != (Path(record) / "hashes.json").read_bytes():
            raise ValueError("Archive does not identify the selected sealed record")
    return {"path": str(archive), "sha256": sha}


def establish(records, reviewer, reason, margin=1.25):
    if len(records) < 2 or len({Path(p).resolve() for p in records}) != len(records):
        raise ValueError("Noise needs at least two distinct records")
    if not reviewer.strip() or not reason.strip() or not math.isfinite(margin) or margin < 1:
        raise ValueError("Need reviewer, reason and a finite margin >=1")
    manifests = []
    for record in records:
        verify_record(record)
        manifests.append(read(Path(record) / "manifest.json"))
    if len({m["source_commit"] for m in manifests}) != 1:
        raise ValueError("Noise records must measure the same source commit")
    identities = [m["instrumentation"].get("patch_sha256") for m in manifests]
    if len(set(identities)) != 1:
        raise ValueError("Noise records must use identical instrumentation")
    comparisons = [compare(Path(a), Path(b), 0) for a, b in itertools.combinations(records, 2)]
    measured = {}
    for comparison in comparisons:
        for row in comparison["results"]:
            if row["direction"] == "descriptive":
                continue
            key = (row["step"], row["metric"], row["unit"], row["direction"])
            before, after = row["base_median"], row["candidate_median"]
            if min(before, after) < 0:
                raise ValueError("Noise calibration requires nonnegative metrics")
            if min(before, after) == 0 and before != after:
                raise ValueError("A changing zero metric needs an absolute policy: " + row["metric"])
            relative = abs(after - before) / min(before, after) if before and after else 0
            measured[key] = max(measured.get(key, 0), relative)
    if not measured:
        raise ValueError("No directional metrics to calibrate")
    for record in records:
        for row in read(Path(record) / "results/summary.json"):
            if row["direction"] == "descriptive":
                continue
            if row["count"] < 3:
                raise ValueError("Need at least three measured repetitions per metric")
            # Preserve within-record spread as well as between-record medians.
            if row["min"] == 0 and row["max"] != 0:
                raise ValueError("A changing zero metric needs an absolute policy: " + row["name"])
            spread = (row["max"] - row["min"]) / row["min"] if row["min"] else 0
            key = (row["step"], row["name"], row["unit"], row["direction"])
            measured[key] = max(measured[key], spread)
    return {"format_version": 1, "status": "reviewed-reference", "source_commit": manifests[0]["source_commit"],
            "reviewer": reviewer, "reason": reason, "margin": margin,
            "tolerance": max(measured.values()) * margin,
            "scope": "Only the exact scenario/environment/build/instrumentation compatibility in these records",
            "limitations": "Observed range with an explicit review margin, not a confidence interval or cross-machine threshold",
            "records": [{"path": str(Path(p).resolve()), "hashes_sha256": digest(Path(p) / "hashes.json")}
                        for p in records],
            "metrics": [{"step": key[0], "name": key[1], "unit": key[2], "direction": key[3],
                         "observed_relative_noise": value} for key, value in sorted(measured.items())]}


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("records", nargs="+")
    ap.add_argument("--output", required=True)
    ap.add_argument("--reviewer", required=True)
    ap.add_argument("--reason", required=True)
    ap.add_argument("--margin", type=float, default=1.25)
    ap.add_argument("--archives", nargs="+", required=True,
                    help="Verified run archives, one per record in the same order")
    args = ap.parse_args()
    destination = Path(args.output).resolve()
    if destination.exists() or any(destination.is_relative_to(Path(p).resolve()) for p in args.records):
        ap.error("Descriptor must be new and outside sealed records")
    try:
        if len(args.archives) != len(args.records):
            raise ValueError("Need one verified archive for each record")
        descriptor = establish(args.records, args.reviewer, args.reason, args.margin)
        for record, archive in zip(descriptor["records"], args.archives):
            record["archive"] = archive_identity(record["path"], archive)
        write(destination, descriptor)
        print(json.dumps(descriptor, indent=2))
    except (OSError, ValueError, KeyError) as exc:
        ap.exit(1, str(exc) + "\n")
