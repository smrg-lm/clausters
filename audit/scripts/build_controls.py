#!/usr/bin/env python3
"""Build unmodified controls in separate targets with retained command receipts."""
import argparse
import os
from pathlib import Path
import shutil

from audit import command, digest, execute, read, write


def build(work, output):
    work, output = Path(work).resolve(), Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = work / "source"
    prepared = read(work / "prepared.json")
    if command(["git", "rev-parse", "HEAD"], source) != prepared["source_commit"] or command(["git", "status", "--porcelain"], source):
        raise ValueError("Control source must be the clean prepared revision")
    receipts = []
    for role, relative, binary in (("server", ".", "clausters"), ("gui", "clients/gui", "clausters-gui")):
        target = work / ("control-target-" + role)
        env = {**os.environ, "CARGO_TARGET_DIR": str(target)}
        # Use the recorded real Cargo, bypassing the instrumented clone's router.
        argv = [env.get("AUDIT_REAL_CARGO", shutil.which("cargo")), "build", "--release", "--locked", "--offline", "--bin", binary]
        outcome = execute(argv, source / relative, env, 3600, output / (role + ".stdout"), output / (role + ".stderr"))
        item = {"role": role, "argv": argv, "cwd": str(source / relative), **outcome}
        receipts.append(item)
        write(output / "receipt.json", receipts)
        if outcome["status"] != "passed":
            raise ValueError("Control build failed: " + role)
        destination = output / binary
        shutil.copy2(target / "release" / binary, destination)
        item.update(path=str(destination), sha256=digest(destination), source_commit=prepared["source_commit"])
        write(output / "receipt.json", receipts)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    build(args.work, args.output)
