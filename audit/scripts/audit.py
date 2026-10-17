#!/usr/bin/env python3
"""Isolated, standard-library audit runner. See audit/README.md."""

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import shlex
import signal
import statistics
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import venv

sys.path.insert(0, str(Path(__file__).resolve().parent))
from controls import policy_environment, validate_policy, target_for, web_preflight, receipt_environment, validate_build_contract

AUDIT = Path(__file__).resolve().parents[1]
REPO = AUDIT.parent
STATES = {"passed", "failed", "skipped", "error"}


def now():
    return datetime.now(timezone.utc).isoformat()


def read(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")
    temporary.replace(path)


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def command(argv, cwd=None, env=None):
    return subprocess.check_output(argv, cwd=cwd, env=env, text=True,
                                   stderr=subprocess.STDOUT).strip()


def safe_child(base, name):
    path = (base / name).resolve()
    if not path.is_relative_to(base.resolve()) or path == base.resolve():
        raise ValueError(f"Unsafe relative path: {name}")
    return path


@contextmanager
def locked(work):
    path = work / ".audit-lock"
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        os.write(fd, str(os.getpid()).encode())
        yield
    finally:
        os.close(fd)
        path.unlink()


def external(path):
    path = Path(path).expanduser().resolve()
    if path.is_relative_to(REPO):
        raise ValueError("Work root must be outside the audit repository")
    # Reject any existing checkout, including the maintainer's main worktree.
    ancestor = path
    while not ancestor.exists():
        ancestor = ancestor.parent
    probe = subprocess.run(["git", "-C", str(ancestor), "rev-parse", "--show-toplevel"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if probe.returncode == 0:
        raise ValueError("Work root must be outside all Git checkouts")
    return path


# The reviewed tool profile; the contract tests point it at a synthetic one.
TOOL_PROFILE = AUDIT / "tools/versions.json"


def tools():
    expected = read(TOOL_PROFILE)["selected_tools"]
    found = []
    for tool in expected:
        executable = shutil.which(tool["command"][0])
        if not executable:
            raise ValueError(f"Missing prerequisite: {tool['name']}")
        output = command(tool["command"])
        if output != tool["version_output"]:
            raise ValueError(f"Tool version mismatch for {tool['name']}: {output!r}; "
                             "create a reviewed tool profile before running")
        found.append({**tool, "executable": executable,
                      "executable_sha256": digest(executable)})
    return found


def expand(argument, substitutions):
    for key, value in substitutions.items():
        argument = argument.replace("{" + key + "}", value)
    return argument


def inspect_environment():
    def contents(path):
        try:
            return Path(path).read_text().strip()
        except OSError:
            return None
    stable = {"platform": platform.platform(), "machine": platform.machine(),
              "cpu_count": os.cpu_count(), "cpuinfo": contents("/proc/cpuinfo"),
              "kernel": platform.release(), "tools": tools()}
    # cpu MHz changes continually; it is a capture, not a comparison key.
    stable["cpuinfo"] = "\n".join(line for line in (stable["cpuinfo"] or "").splitlines()
                                    if not line.startswith("cpu MHz"))
    environment = {"format_version": 1, "captured_at_utc": now(), "identity": stable,
                   "memory": contents("/proc/meminfo"),
                   "load": contents("/proc/loadavg"),
                   "process_limits": contents("/proc/self/limits"),
                   "affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
                   "governor": contents("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
                   "unavailable_fields_with_reasons": [
                       "GPU/driver/display/audio configuration: supply in manual evidence when used",
                       "Temperature, throttling, actual audio-thread policy: manual capture when applicable"]}
    return environment


def package_inventory(source):
    names = command(["git", "ls-files", "--", "*package.json", "*pyproject.toml"], source).splitlines()
    result = []
    for name in sorted(names):
        path = Path(source) / name
        if path.name == "package.json":
            data = read(path)
        else:
            with path.open("rb") as stream:
                data = tomllib.load(stream).get("project", {})
        if not data.get("name"):
            continue
        unit = "editor" if name.startswith("clausters-editor/") else "python" if name.startswith("clients/python/") else "web" if name.startswith("clients/web/") else "unassigned"
        result.append({"manifest": name, "package": data["name"], "sha256": digest(path), "unit": unit})
    return result


def link_targets(source, target):
    for relative in (".", "clients/gui"):
        parent = source / relative
        link = parent / "target"
        if parent.is_dir() and not link.exists() and not link.is_symlink():
            link.symlink_to(target_for(target, relative), target_is_directory=True)


def crate_inventory(source):
    paths = command(["git", "ls-files", "--", "*Cargo.toml"], source).splitlines()
    result = []
    for name in sorted(paths):
        path = Path(source) / name
        if not path.exists():
            continue
        with path.open("rb") as stream:
            data = tomllib.load(stream)
            package = data.get("package")
        if package:
            result.append({"manifest": name, "package": package["name"], "sha256": digest(path),
                           "unit": "editor" if Path(name).parts[0] == "clausters-editor" else "components",
                           "declared_features": sorted(data.get("features", {}))})
    return result


def prepare(work, revision, policy=None):
    policy = validate_policy(policy or {})
    work = external(work)
    work.mkdir(parents=True, exist_ok=False)
    with locked(work):
        policy_environment(policy, work)
        selected_tools = tools()
        source = command(["git", "rev-parse", f"{revision}^{{commit}}"], REPO)
        resources = command(["git", "rev-parse", "HEAD"], REPO)
        if command(["git", "status", "--porcelain", "--", "audit"], REPO):
            raise ValueError("Commit audit resources before preparing a real campaign")
        checkout = work / "source"
        command(["git", "clone", "--no-hardlinks", "--no-checkout", str(REPO), str(checkout)])
        command(["git", "checkout", "--detach", source], checkout)
        link_targets(checkout, work / "target")
        venv.EnvBuilder(with_pip=False).create(work / "tools/python")
        # A copy of all runner resources survives later edits to the audit branch.
        resource_names = subprocess.check_output(
            ["git", "ls-files", "-z", "--", "audit/"], cwd=REPO).decode().split("\0")
        for name in filter(None, resource_names):
            relative = Path(name).relative_to("audit")
            original = AUDIT / relative
            if original.is_symlink():
                raise ValueError("Audit resources cannot contain symlinks")
            destination = work / "resources" / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, destination)
        router = work / "tools/router/cargo"
        router.parent.mkdir(parents=True)
        router.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " " +
                          shlex.quote(str(work / "resources/scripts/cargo_router.py")) + ' "$@"\n')
        router.chmod(0o755)
        if command(["git", "status", "--porcelain"], checkout):
            raise ValueError("Preparation produced unignored files")
        source_archive = work / "source.tar"
        subprocess.run(["git", "archive", "--format=tar", f"--output={source_archive}", source],
                       cwd=checkout, check=True)
        resource_archive = work / "resources.tar"
        with tarfile.open(resource_archive, "w") as tar:
            tar.add(work / "resources", arcname="audit")
        reproduction_inputs = [{"name": "source.tar", "path": str(source_archive), "sha256": digest(source_archive)},
                               {"name": "resources.tar", "path": str(resource_archive), "sha256": digest(resource_archive)}]
        write(work / "prepared.json", {"format_version": 1, "reproduction_inputs": reproduction_inputs, "source_commit": source,
              "audit_resources_commit": resources, "prepared_at_utc": now(), "policy": policy,
              "resource_files": {str(p.relative_to(work / "resources")): digest(p) for p in sorted((work / "resources").rglob("*")) if p.is_file()},
              "tools": selected_tools, "source": str(checkout), "crates": crate_inventory(checkout), "packages": package_inventory(checkout),
              "lockfiles": {str(p.relative_to(checkout)): digest(p)
                            for p in [checkout / n for n in ("Cargo.lock", "clients/gui/Cargo.lock", "clausters-editor/src-tauri/Cargo.lock", "clients/web/package-lock.json", "clausters-editor/package-lock.json", "clients/python/pyproject.toml")] if p.exists()}})
    print(work / "prepared.json")


def setup_unit(work, unit, wheelhouse=None):
    work = Path(work).resolve()
    prepared = read(work / "prepared.json")
    with locked(work):
        env = {**os.environ, "CARGO_TARGET_DIR": str(work / "target"),
               "CARGO_NET_OFFLINE": "true", "npm_config_cache": str(work / "tools/npm-cache"),
               "npm_config_offline": "true"}
        env.update(policy_environment(prepared.get("policy", {}), work))
        tools()
        source = work / "source"
        env.update(AUDIT_SOURCE=str(source), AUDIT_TARGET=str(work / "target"), AUDIT_REAL_CARGO=shutil.which("cargo") or "cargo")
        env["PATH"] = str(work / "tools/router") + os.pathsep + env["PATH"]
        for key in list(env):
            if key.upper().startswith("PIP_"):
                del env[key]
        env["PIP_CONFIG_FILE"] = os.devnull
        env["PIP_DISABLE_PIP_VERSION_CHECK"] = "1"
        env["PYTHONNOUSERSITE"] = "1"
        commands = []
        if unit == "python":
            if not wheelhouse or not Path(wheelhouse).is_dir():
                raise ValueError("Python setup needs an explicit offline wheelhouse")
            destination = work / "tools/client-python"
            if destination.exists():
                raise ValueError("Client environment already exists; use a new preparation after failed setup")
            venv.EnvBuilder(with_pip=True).create(destination)
            python = str(destination / "bin/python")
            commands.append(([python, "-m", "pip", "install", "--no-index", "--no-deps",
                              "--find-links", str(Path(wheelhouse).resolve()), "-r",
                              str(work / "resources/tools/python-requirements.txt")], source))
            commands.append(([python, "-m", "pip", "check"], source))
            commands.append(([python, "-m", "pip", "freeze", "--all"], source))
        else:
            relative = "clients/web" if unit == "web" else "clausters-editor"
            commands.append((["npm", "ci", "--offline", "--audit=false", "--fund=false"], source / relative))
            # Existing web staging and Python builders resolve workspace/target literally.
            for parent in (source, source / "clients/gui"):
                link = parent / "target"
                if not link.exists() and not link.is_symlink():
                    link.symlink_to(target_for(work / "target", str(parent.relative_to(source))), target_is_directory=True)
        receipt = []
        directory = work / "setup" / unit
        directory.mkdir(parents=True, exist_ok=False)
        for index, (argv, cwd) in enumerate(commands):
            env.update(policy_environment(prepared.get("policy", {}), work))
            outcome = execute(argv, cwd, env, 1800, directory / f"{index}.stdout", directory / f"{index}.stderr")
            receipt.append({"argv": argv, "cwd": str(cwd), "environment": receipt_environment(env), **outcome})
            write(directory / "receipt.json", receipt)
            if outcome["status"] != "passed":
                raise ValueError(f"Unit setup failed; inspect {directory}")
        if wheelhouse:
            write(directory / "wheelhouse.json", {p.name: digest(p) for p in sorted(Path(wheelhouse).glob("*.whl"))})
        print(directory / "receipt.json")


def validate_scenario(value):
    if value.get("format_version") != 1 or value.get("status") != "ready":
        raise ValueError("Scenario must have format_version=1 and status=ready")
    if not re.fullmatch(r"[a-z][a-z0-9_-]*", value.get("id", "")):
        raise ValueError("Invalid scenario ID")
    if not value.get("steps"):
        raise ValueError("Scenario needs steps")
    names = set()
    for step in value["steps"]:
        if not re.fullmatch(r"[a-z][a-z0-9_-]*", step.get("id", "")) or step["id"] in names:
            raise ValueError("Step IDs must be unique safe names")
        dependencies = step.get("depends_on", [])
        if not isinstance(dependencies, list) or any(d not in names for d in dependencies):
            raise ValueError("Dependencies must name earlier steps (no cycles or unknown IDs)")
        names.add(step["id"])
        for filename in step.get("generated_outputs", []) + step.get("retain_outputs", []):
            if not isinstance(filename, str) or Path(filename).is_absolute() or ".." in Path(filename).parts:
                raise ValueError("Output paths must be source-relative")
        if step.get("isolate") not in {None, "pytest-file"}:
            raise ValueError("Unknown isolation mode")
        if step.get("compatibility_capture") not in {None, "device-paired"}:
            raise ValueError("Unknown compatibility capture")
        if step.get("profile", "debug") not in {"debug", "release"}:
            raise ValueError("Unsupported profile")
        if step.get("features") is not None and (not isinstance(step["features"], list) or
                not all(isinstance(f, str) and re.fullmatch(r"[a-zA-Z0-9_/-]+", f) for f in step["features"])):
            raise ValueError("Invalid features")
        cwd = Path(step.get("cwd", "."))
        if cwd.is_absolute() or ".." in cwd.parts:
            raise ValueError("Command cwd must be relative and cannot traverse parents")
        if step.get("kind") not in {"command", "manual", "sweep"}:
            raise ValueError("Unknown step kind")
        if not isinstance(step.get("required"), bool):
            raise ValueError("Step needs an explicit required flag")
        if step["kind"] in {"command", "sweep"}:
            if step["kind"] == "sweep":
                from device import validate_sweep
                validate_sweep(step)
            argv = step.get("argv")
            if not isinstance(argv, list) or not argv or not all(isinstance(x, str) for x in argv):
                raise ValueError("Commands must be nonempty argument arrays")
            for key in ("repetitions", "timeout_seconds"):
                if type(step.get(key)) is not int or step[key] <= 0:
                    raise ValueError(f"Invalid {key}")
            if type(step.get("warmup")) is not int or step["warmup"] < 0:
                raise ValueError("Invalid warmup count")
            if step.get("parser") not in {"exit", "rust-tests", "bench", "metrics", "pytest", "node-tests"}:
                raise ValueError("Unknown parser")


def execute(argv, cwd, env, timeout, stdout_path, stderr_path):
    if env.get("AUDIT_CPUS"):
        argv = ["taskset", "--cpu-list", env["AUDIT_CPUS"], *argv]
    started = time.perf_counter_ns()
    disk_before = shutil.disk_usage(stdout_path.parent).free
    if disk_before < float(env.get("AUDIT_DISK_FLOOR_GIB", 0)) * 1024 ** 3:
        raise ValueError("Free disk space is below campaign floor")
    status, code, termination = "error", None, None
    with stdout_path.open("wb") as out, stderr_path.open("wb") as err:
        try:
            process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err,
                                       start_new_session=True)
        except OSError as exc:
            err.write(str(exc).encode())
            termination = "launch-error"
        else:
            try:
                code = process.wait(timeout=timeout)
                status = "passed" if code == 0 else "failed"
            except (subprocess.TimeoutExpired, KeyboardInterrupt) as exc:
                os.killpg(process.pid, signal.SIGKILL)
                code = process.wait()
                termination = "timeout" if isinstance(exc, subprocess.TimeoutExpired) else "interrupted"
    output = stdout_path.read_text(errors="replace") + "\n" + stderr_path.read_text(errors="replace")
    tests = re.findall(r"(?:test [^\n]+ \.{3}|[^\n ]+::[^\n]+)", output)
    return {"status": status, "exit_code": code, "effective_argv": argv,
            "termination": termination,
            "disk_before_bytes": disk_before,
            "disk_after_bytes": shutil.disk_usage(stdout_path.parent).free,
            "signal": signal.Signals(-code).name if code is not None and code < 0 else None,
            "last_test": tests[-1] if tests else None,
            "subprocess_signals": re.findall(r"isolated signal: (SIG[A-Z0-9]+)", output),
            "elapsed_seconds": (time.perf_counter_ns() - started) / 1e9}


def metric(name, unit, value, direction="descriptive"):
    if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value):
        raise ValueError(f"Non-finite or nonnumeric metric: {name}")
    if direction not in {"lower", "higher", "descriptive"}:
        raise ValueError("Invalid metric direction")
    if not isinstance(name, str) or not name or not isinstance(unit, str) or not unit:
        raise ValueError("Metric name and unit must be nonempty strings")
    return {"name": name, "unit": unit, "value": value, "direction": direction}


def parse_metrics(parser, output, allow_failed=False):
    if parser == "rust-tests":
        counts = re.findall(r"test result: (?:ok|FAILED)\. (\d+) passed", output)
        if not counts or (sum(map(int, counts)) == 0 and not allow_failed):
            raise ValueError("No passing Rust tests observed; empty suite is not acceptance")
        return [metric("tests_passed", "count", sum(map(int, counts))),
                metric("tests_failed", "count", sum(map(int, re.findall(r"(\d+) failed", output))))]
    if parser in {"pytest", "node-tests"}:
        if parser == "pytest":
            passed = re.findall(r"(\d+) passed", output)
            skipped = re.findall(r"(\d+) (?:skipped|xfailed|xpassed)", output)
        else:
            passed = re.findall(r"(?:#|\u2139)\s+pass\s+(\d+)", output)
            skipped = re.findall(r"(?:#|\u2139)\s+(?:skipped|todo|cancelled)\s+(\d+)", output)
        if (not passed or sum(map(int, passed)) == 0) and not allow_failed:
            raise ValueError("No passing tests observed")
        return [metric("tests_passed", "count", sum(map(int, passed))),
                metric("tests_skipped", "count", sum(map(int, skipped))),
                metric("tests_failed", "count", sum(map(int, re.findall(r"(\d+) failed", output))))]
    if parser == "bench":
        try:
            rows = json.loads(output)
            if isinstance(rows, dict):
                rows = [rows]
        except ValueError:
            rows = [json.loads(line) for line in output.splitlines() if line.startswith("{")]
        if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
            raise ValueError("Benchmark must emit an array of records or JSONL records")
        if not rows:
            raise ValueError("Benchmark produced no JSON records")
        result = []
        for row in rows:
            for key, unit, direction in (("x_real_time", "ratio", "higher"),
                                         ("peak_block", "microseconds", "lower")):
                if row.get(key) is not None:
                    # Existing benchmark rows excluded by their own gate remain descriptive.
                    result.append(metric(row["name"] + "." + key, unit, row[key],
                                         direction if row.get("gated") is True else "descriptive"))
        if not result:
            raise ValueError("Benchmark contains no supported metric")
        return result
    if parser == "metrics":
        rows = [json.loads(line) for line in output.splitlines() if line.strip()]
        if not rows:
            raise ValueError("Measurement driver produced no metrics")
        return [metric(row["name"], row["unit"], row["value"], row.get("direction", "descriptive")) for row in rows]
    return []


def summarize(rows):
    groups = {}
    for row in rows:
        checked = metric(row["name"], row["unit"], row["value"], row["direction"])
        key = (row["step"], checked["name"], checked["unit"], checked["direction"])
        groups.setdefault(key, []).append(checked["value"])
    result = []
    for (step, name, unit, direction), values in sorted(groups.items()):
        ordered = sorted(values)
        result.append({"step": step, "name": name, "unit": unit, "direction": direction,
                       "count": len(values), "median": statistics.median(values),
                       "min": ordered[0], "max": ordered[-1],
                       "p95": ordered[max(0, math.ceil(.95 * len(values)) - 1)]})
    return result


def load_rows(run):
    return [json.loads(line) for line in (run / "results/measurements.jsonl").read_text().splitlines() if line]


def append_rows(run, rows):
    with (run / "results/measurements.jsonl").open("a") as out:
        for row in rows:
            out.write(json.dumps(row, allow_nan=False) + "\n")


def refresh(run):
    manifest, checks = read(run / "manifest.json"), read(run / "results/checks.json")
    summary = summarize(load_rows(run))
    write(run / "results/summary.json", summary)
    failed = any(c["status"] in {"failed", "error"} for c in checks)
    missing = any(c["required"] and c["status"] != "passed" for c in checks)
    manifest["status"] = "failed" if failed else "partial" if missing else "needs_review"
    manifest["finished_at_utc"] = now()
    write(run / "manifest.json", manifest)
    lines = ["# Campaign report", "", f"Status: {manifest['status']}", "",
             f"Source: `{manifest['source_commit']}`", "",
             "Durations around commands include process startup and are not audio callback or GUI frame latency.",
             "", "## Checks", ""]
    lines += [f"- {c['step']}: {c['status']} -- {c.get('reason', '')}" for c in checks]
    lines += ["", "## Measurements", "", "| Step / metric | Unit | n | Median | Min | Max |", "| --- | --- | --- | --- | --- | --- |"]
    lines += [f"| {s['step']} / {s['name']} | {s['unit']} | {s['count']} | {s['median']:.6g} | {s['min']:.6g} | {s['max']:.6g} |" for s in summary]
    lines += ["", "See scenario snapshots, logs, manual evidence and findings.md for context. p95 uses nearest-rank over repetitions; it is not a per-callback or per-frame quantile.", ""]
    (run / "report.md").write_text("\n".join(lines))


def run_campaign(work, scenario_path, name, patch=None):
    work = Path(work).resolve()
    prepared = read(work / "prepared.json")
    if digest(AUDIT / "scripts/audit.py") != digest(work / "resources/scripts/audit.py"):
        raise ValueError("Run using the prepared resources/scripts/audit.py snapshot")
    for filename, expected in prepared.get("resource_files", {}).items():
        if filename.startswith(("scripts/", "instrumentation/")) and digest(AUDIT / filename) != expected:
            raise ValueError("Run using the complete prepared resource snapshot: " + filename)
    scenario = read(scenario_path)
    validate_scenario(scenario)
    if not re.fullmatch(r"[a-zA-Z0-9_-]+", name):
        raise ValueError("Campaign name must use letters, digits, underscores or hyphens")
    with locked(work):
        environment = inspect_environment()
        if environment["identity"]["tools"] != prepared["tools"]:
            raise ValueError("Tool executables changed since preparation")
        source = work / "source"
        if command(["git", "rev-parse", "HEAD"], source) != prepared["source_commit"]:
            raise ValueError("Measurement checkout moved from prepared revision")
        if command(["git", "status", "--porcelain"], source):
            raise ValueError("Unrecorded checkout modifications; export/verify instrumentation first")
        instrumentation = {"used": False}
        if patch:
            patch = Path(patch).resolve()
            instrumentation = {"used": True, "base_commit": prepared["source_commit"],
                               **replay(source, prepared["source_commit"], patch)}
            isolated = work / "instrumented" / name
            isolated.parent.mkdir(exist_ok=True)
            command(["git", "clone", "--no-hardlinks", "--no-checkout", str(source), str(isolated)])
            command(["git", "checkout", "--detach", prepared["source_commit"]], isolated)
            command(["git", "apply", "--index", str(patch)], isolated)
            source = isolated
            link_targets(source, work / "target")
        campaign = safe_child(work / "runs", name)
        campaign.mkdir(parents=True, exist_ok=False)
        for directory in ("logs", "results", "scenarios", "evidence"):
            (campaign / directory).mkdir()
        (campaign / "results/measurements.jsonl").touch()
        if patch:
            shutil.copyfile(patch, campaign / "instrumentation.patch")
        if (work / "setup").exists():
            shutil.copytree(work / "setup", campaign / "evidence/setup")
        write(campaign / "environment.json", environment)
        write(campaign / "scenarios" / (scenario["id"] + ".json"), scenario)
        compatibility = {"scenario": scenario, "environment": environment["identity"],
                         "governor": environment["governor"], "affinity": environment["affinity"],
                         "policy": prepared.get("policy", {})}
        manifest = {**prepared, "campaign_id": name, "status": "running",
                    "started_at_utc": now(), "finished_at_utc": None,
                    "compatibility": compatibility, "commands": [],
                    "instrumentation": instrumentation, "crates": crate_inventory(source), "packages": package_inventory(source), "external_archives": []}
        write(campaign / "manifest.json", manifest)
        (campaign / "findings.md").write_text("# Findings\n\nNo findings reviewed yet.\n")
        substitutions = {"source": str(source), "target": str(work / "target"),
                         "gui_target": str(work / "target-gui"),
                         "work": str(work), "audit": str(work / "resources"),
                         "python": str(work / "tools/python/bin/python"),
                         "client_python": str(work / "tools/client-python/bin/python")}
        env = os.environ.copy()
        # Keep ordinary compiler/linker configuration, record it below; avoid inherited artifact overrides.
        for key in list(env):
            if key.startswith("CLAUSTERS_"):
                del env[key]
        env.update(policy_environment(prepared.get("policy", {}), work))
        env.update({"CARGO_TARGET_DIR": str(work / "target"), "PYTHONDONTWRITEBYTECODE": "1",
                    "CARGO_NET_OFFLINE": "true", "npm_config_offline": "true",
                    "npm_config_cache": str(work / "tools/npm-cache"),
                    "PYTHONPATH": str(source / "clients/python")})
        env.update(AUDIT_SOURCE=str(source), AUDIT_TARGET=str(work / "target"), AUDIT_REAL_CARGO=shutil.which("cargo") or "cargo")
        env["PATH"] = str(work / "tools/router") + os.pathsep + str(work / "tools/client-python/bin") + os.pathsep + str(work / "tools/cargo/bin") + os.pathsep + env["PATH"]
        if scenario.get("unit") == "python":
            env["CLAUSTERS_REQUIRE_COMPLETE"] = "1"
        manifest["build_environment"] = {key: env.get(key) for key in (
            "RUSTFLAGS", "RUSTDOCFLAGS", "CC", "CXX", "FAUST_PREFIX", "VEROVIO_PREFIX", "RUSTUP_TOOLCHAIN",
            "CARGO_BUILD_JOBS", "RUST_TEST_THREADS", "CARGO_INCREMENTAL", "BINDGEN_EXTRA_CLANG_ARGS", "LIBCLANG_PATH")}
        input_names = set(prepared["lockfiles"]) | {item["manifest"] for item in manifest["crates"] + manifest["packages"]}
        locked_inputs = {name: digest(source / name) for name in sorted(input_names) if (source / name).is_file()}
        manifest["effective_lockfiles"] = locked_inputs
        checks = []
        for step in scenario["steps"]:
            if step["kind"] == "manual":
                checks.append({"step": step["id"], "required": step["required"], "status": "skipped",
                               "reason": step["instructions"]})
                write(campaign / "results/checks.json", checks)
                continue
            if any(not any(c["step"] == dependency and c["status"] == "passed" for c in checks)
                   or any(c["step"] == dependency and c["status"] != "passed" for c in checks)
                   for dependency in step.get("depends_on", [])):
                checks.append({"step": step["id"], "required": step["required"], "status": "skipped",
                               "reason": "Declared prerequisite failed or skipped; stale outputs must not be measured"})
                write(campaign / "results/checks.json", checks)
                continue
            step_env = env.copy()
            target = target_for(work / "target", step.get("workspace", step.get("cwd", ".")))
            step_env["CARGO_TARGET_DIR"] = str(target)
            step_substitutions = {**substitutions, "target": str(target), "output": str(campaign / "evidence" / step["id"])}
            argv = [expand(arg, step_substitutions) for arg in step["argv"]]
            if step.get("isolate") == "pytest-file":
                argv = [sys.executable, str(AUDIT / "scripts/isolate_pytest.py"), *argv]
            cwd = safe_child(source, step.get("cwd", ".")) if step.get("cwd", ".") != "." else source
            if not cwd.is_dir():
                checks.append({"step": step["id"], "required": step["required"],
                               "status": "error", "reason": f"Missing working directory: {cwd}"})
                write(campaign / "results/checks.json", checks)
                continue
            for index in range(-step["warmup"], step["repetitions"]):
                prefix = f"{step['id']}-{index}"
                stdout = campaign / "logs" / (prefix + ".stdout")
                stderr = campaign / "logs" / (prefix + ".stderr")
                entry = {"step": step["id"], "repetition": index, "argv": argv, "cwd": str(cwd), "started_at_utc": now(), "environment": receipt_environment(step_env),
                         "workspace": step.get("workspace", step.get("cwd", ".")),
                         "profile": step.get("profile", "debug"), "target_dir": str(target)}
                try:
                    step_env.update(policy_environment(prepared.get("policy", {}), work))
                    entry["environment"] = receipt_environment(step_env)
                    validate_build_contract(step, source)
                    if step.get("preflight") == "web":
                        write(campaign / "evidence/web-assets.json", web_preflight(source))
                    if step["kind"] == "sweep":
                        from device import execute_sweep
                        outcome = execute_sweep(step, argv, cwd, step_env, campaign, index, execute)
                    else:
                        outcome = execute(argv, cwd, step_env, step["timeout_seconds"], stdout, stderr)
                    entry.update(outcome)
                    executable = shutil.which(argv[0], path=env["PATH"])
                    if executable:
                        entry["executable"] = executable
                        entry["executable_sha256"] = digest(executable)
                    changed = [name for name, expected in locked_inputs.items()
                               if not (source / name).is_file() or digest(source / name) != expected]
                    if changed:
                        raise ValueError("Build changed a recorded manifest/lockfile: " + ", ".join(changed))
                    measured = []
                    output = stdout.read_text(errors="replace")
                    if step["parser"] in {"rust-tests", "pytest", "node-tests"}:
                        output += "\n" + stderr.read_text(errors="replace")
                    try:
                        measured = parse_metrics(step["parser"], output, allow_failed=outcome["status"] != "passed")
                    except ValueError:
                        if outcome["status"] == "passed":
                            raise
                    entry["observed_metrics"] = measured
                    if outcome["status"] == "passed" and step.get("compatibility_capture") == "device-paired":
                        from paired import compatibility as device_compatibility
                        manifest["compatibility"]["device"] = device_compatibility(campaign / "evidence" / step["id"])
                    artifacts = []
                    for line in output.splitlines():
                        try:
                            item = json.loads(line)
                        except ValueError:
                            continue
                        if isinstance(item, dict) and item.get("reason") == "compiler-artifact":
                            artifacts.append({key: item.get(key) for key in (
                                "package_id", "manifest_path", "target", "features", "profile", "executable", "filenames", "fresh")})
                    entry["compiled_targets"] = artifacts
                    if index >= 0 and outcome["status"] != "passed" and measured:
                        append_rows(campaign, [{**m, "step": step["id"], "repetition": index} for m in measured])
                    if index >= 0 and outcome["status"] == "passed":
                        measured += [metric("command_wall_seconds", "seconds", outcome["elapsed_seconds"])]
                        append_rows(campaign, [{**m, "step": step["id"], "repetition": index} for m in measured])
                    reason = "Exit status and parser assertions" if outcome["status"] == "passed" else "Command failed or timed out; inspect logs"
                    if entry["status"] == "passed" and any(m["name"] == "tests_skipped" and m["value"] > 0 for m in measured):
                        entry["status"] = "skipped"
                        reason = "Suite skipped tests; inspect logs and complete prerequisites instead of accepting partial coverage"
                except (OSError, ValueError, KeyError) as exc:
                    entry["status"] = "error"
                    entry.setdefault("exit_code", None)
                    reason = str(exc)
                generated = step.get("generated_outputs", [])
                if generated:
                    for filename in generated:
                        safe_child(source, filename)
                    diff = subprocess.check_output(["git", "diff", "--binary", "--", *generated], cwd=source)
                    (campaign / "evidence" / f"{prefix}-generated.patch").write_bytes(diff)
                    for filename in generated:
                        original = subprocess.check_output(["git", "show", f":{filename}"], cwd=source)
                        (source / filename).write_bytes(original)
                for filename in step.get("retain_outputs", []):
                    original = safe_child(source, filename)
                    if original.is_file():
                        destination = campaign / "evidence" / prefix / filename
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copyfile(original, destination)
                        if filename.endswith("Cargo.lock"):
                            locked_inputs[filename] = digest(original)
                manifest["commands"].append(entry)
                checks.append({"step": step["id"], "repetition": index,
                               "required": step["required"], "status": entry["status"], "reason": reason})
                write(campaign / "manifest.json", manifest)
                write(campaign / "results/checks.json", checks)
                if entry["status"] != "passed":
                    break
        refresh(campaign)
    print(campaign)
    status = read(campaign / "manifest.json")["status"]
    return 1 if status == "failed" else 3 if status == "partial" else 0


def export_patch(source, base, output):
    source, output = Path(source).resolve(), Path(output).resolve()
    base = command(["git", "rev-parse", f"{base}^{{commit}}"], source)
    if output.is_relative_to(source):
        raise ValueError("Patch output must be outside measurement checkout")
    with tempfile.TemporaryDirectory() as temporary:
        env = {**os.environ, "GIT_INDEX_FILE": str(Path(temporary) / "index")}
        command(["git", "read-tree", base], source, env)
        command(["git", "add", "-A", "--", "."], source, env)
        cached = subprocess.check_output(["git", "ls-files", "-z"], cwd=source).split(b"\0")
        existing = [name for name in cached if name and os.path.lexists(os.fsencode(source) + b"/" + name)]
        if existing:
            subprocess.run(["git", "add", "--force", "--pathspec-from-file=-", "--pathspec-file-nul"],
                           cwd=source, env=env, input=b"\0".join(existing) + b"\0", check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        patch = subprocess.check_output(["git", "diff", "--cached", "--binary", "--full-index", base], cwd=source, env=env)
        output.write_bytes(patch)
    return {"base_commit": base, "patch_sha256": digest(output)}


def replay(repo, base, patch):
    patch = Path(patch).resolve()
    with tempfile.TemporaryDirectory(prefix="clausters-audit-replay-") as temporary:
        source = Path(temporary) / "source"
        command(["git", "clone", "--no-hardlinks", "--no-checkout", str(repo), str(source)])
        command(["git", "checkout", "--detach", base], source)
        if patch.stat().st_size:
            command(["git", "apply", "--check", str(patch)], source)
            command(["git", "apply", "--index", str(patch)], source)
        regenerated = Path(temporary) / "reexport.patch"
        export_patch(source, base, regenerated)
        if regenerated.read_bytes() != patch.read_bytes():
            raise ValueError("Instrumentation replay differs from saved patch")
    return {"patch_sha256": digest(patch), "replay_verified": True}


def verify_record(run):
    hashes = read(run / "hashes.json")
    if any(p.is_symlink() for p in run.rglob("*")):
        raise ValueError("Record cannot contain symlinks")
    actual = {p.relative_to(run).as_posix() for p in run.rglob("*") if p.is_file() and p != run / "hashes.json"}
    if set(hashes) != actual:
        raise ValueError("Record file list differs from hashes.json")
    for name, expected in hashes.items():
        path = safe_child(run, name)
        if path.is_symlink() or digest(path) != expected:
            raise ValueError(f"Evidence checksum mismatch: {name}")
    return True


def seal_record(run, reviewer, preserve_incomplete=False, reason=None):
    manifest = read(run / "manifest.json")
    if (run / "hashes.json").exists():
        raise ValueError("Record already sealed; create a corrected record instead")
    checks = read(run / "results/checks.json")
    scenarios = list((run / "scenarios").glob("*.json"))
    if len(scenarios) != 1:
        raise ValueError("Record must contain exactly one resolved scenario")
    scenario = read(scenarios[0])
    validate_scenario(scenario)
    incomplete = False
    for step in scenario["steps"]:
        entries = [c for c in checks if c["step"] == step["id"]]
        expected = 1 if step["kind"] == "manual" else step["warmup"] + step["repetitions"]
        if any(c["required"] != step["required"] for c in entries):
            raise ValueError("Inconsistent check coverage")
        incomplete = incomplete or len(entries) != expected
    if read(run / "results/summary.json") != summarize(load_rows(run)):
        raise ValueError("Summary differs from raw measurements")
    if manifest["instrumentation"]["used"]:
        info = manifest["instrumentation"]
        if not info.get("replay_verified") or digest(run / "instrumentation.patch") != info["patch_sha256"]:
            raise ValueError("Instrumentation is not replay-verified")
    if any(c["status"] not in STATES for c in checks):
        raise ValueError("Invalid check status")
    incomplete = incomplete or any(c["required"] and c["status"] != "passed" for c in checks)
    failed = any(c["status"] in {"failed", "error"} for c in checks)
    if (incomplete or failed) and not preserve_incomplete:
        raise ValueError("Unfulfilled/failed checks cannot be accepted; use explicit evidence-only preservation")
    if preserve_incomplete and not reason:
        raise ValueError("Evidence-only preservation requires a review reason")
    status = "failed" if failed else "partial" if incomplete else "complete"
    manifest.update({"status": status, "reviewed_by": reviewer, "reviewed_at_utc": now(),
                     "review_outcome": "evidence-only" if preserve_incomplete else "accepted",
                     "review_reason": reason, "quality_accepted": not preserve_incomplete})
    write(run / "manifest.json", manifest)
    report = run / "report.md"
    report.write_text(report.read_text().replace("Status: needs_review", f"Status: {status}") +
                      f"\nReview outcome: {manifest['review_outcome']}. Reason: {reason or 'All required checks passed'}.\n")
    hashes = {}
    for path in sorted(run.rglob("*")):
        if path.is_symlink():
            raise ValueError("Record cannot contain symlinks")
        if path.is_file():
            hashes[path.relative_to(run).as_posix()] = digest(path)
    write(run / "hashes.json", hashes)
    verify_record(run)


def attach(run, step_id, evidence):
    if (run / "hashes.json").exists():
        raise ValueError("Cannot modify sealed record")
    if (run / "evidence" / f"{step_id}.json").exists():
        raise ValueError("Manual evidence already retained; use a new campaign for corrections")
    payload = read(evidence)
    scenario = next((run / "scenarios").glob("*.json"))
    step = next(s for s in read(scenario)["steps"] if s["id"] == step_id)
    if step["kind"] != "manual":
        raise ValueError("Evidence attachment is for manual steps only")
    checks = read(run / "results/checks.json")
    if any(c["step"] == step_id and c["status"] != "skipped" for c in checks):
        raise ValueError("Manual evidence already attached; use a new campaign for corrections")
    if payload.get("status") not in {"passed", "failed", "skipped", "error"}:
        raise ValueError("Invalid manual status")
    for key in ("operator", "procedure", "configuration", "limitations", "reason"):
        if not payload.get(key):
            raise ValueError(f"Manual evidence requires {key}")
    if payload["status"] == "passed" and step_id in {"gui-device", "combined-device", "device-review"}:
        if payload.get("screen_on") is not True:
            raise ValueError("GUI evidence requires screen_on=true throughout capture")
        gui_runs = payload.get("gui_runs", [])
        if not gui_runs:
            raise ValueError("GUI evidence needs per-run frames and elapsed_seconds")
        for trial in gui_runs:
            frames, elapsed = trial.get("frames"), trial.get("elapsed_seconds")
            if (type(frames) is not int or frames < 0 or isinstance(elapsed, bool)
                    or not isinstance(elapsed, (int, float)) or not math.isfinite(elapsed)
                    or elapsed <= 0 or frames / elapsed < 20):
                raise ValueError("GUI run did not draw at least 20 fps; keep screen on")
    if payload["status"] == "passed" and step_id in {"gui-device", "combined-device", "device-review", "web-review"}:
        renderer = payload.get("renderer", {})
        if renderer.get("kind") not in {"software", "hardware"} or not all(renderer.get(k) for k in ("adapter", "driver", "backend")):
            raise ValueError("Rendering evidence needs kind, adapter, driver and backend identity")
        if renderer["kind"] == "software" and payload.get("physical_gpu_acceptance"):
            raise ValueError("Software acceptance cannot establish physical GPU measurements")
    if step_id == "crate-review":
        inventory = {item["manifest"] for item in read(run / "manifest.json")["crates"] if item["unit"] == "components"}
        coverage = payload.get("crate_coverage", {})
        if set(coverage) != inventory:
            raise ValueError("Crate coverage must name every inventoried package manifest exactly")
        for item in coverage.values():
            if item.get("status") not in STATES or not item.get("reason") or not item.get("evidence"):
                raise ValueError("Each crate needs status, rationale and evidence reference")
        if payload["status"] == "passed" and any(item["status"] != "passed" for item in coverage.values()):
            raise ValueError("Unreviewed or unsupported crates cannot pass whole-project coverage")
    if step_id in {"python-review", "web-review", "editor-review"}:
        unit = step_id.removesuffix("-review")
        manifest = read(run / "manifest.json")
        inventory = {item["manifest"] for item in manifest["packages"] + manifest["crates"] if item["unit"] == unit}
        coverage = payload.get("package_coverage", {})
        if set(coverage) != inventory:
            raise ValueError("Package coverage must name every manifest in this audit unit")
        for item in coverage.values():
            if item.get("status") not in STATES or not item.get("reason") or not item.get("evidence"):
                raise ValueError("Each package needs status, rationale and evidence reference")
        if payload["status"] == "passed" and any(item["status"] != "passed" for item in coverage.values()):
            raise ValueError("Unreviewed package manifests cannot pass unit coverage")
    rows = [{**metric(m["name"], m["unit"], m["value"], m.get("direction", "descriptive")),
             "step": step_id, "repetition": i} for i, m in enumerate(payload.get("metrics", []))]
    saved = {**payload, "artifacts": []}
    for i, item in enumerate(payload.get("artifacts", [])):
        original = Path(item).resolve()
        destination = run / "evidence" / f"{step_id}-{i}-{original.name}"
        shutil.copyfile(original, destination)
        saved["artifacts"].append({"path": str(destination.relative_to(run)), "sha256": digest(destination)})
    if payload["status"] == "passed" and not saved["artifacts"]:
        raise ValueError("Passing manual checks require retained artifacts")
    write(run / "evidence" / f"{step_id}.json", saved)
    append_rows(run, rows)
    checks = [c for c in checks if c["step"] != step_id]
    checks.append({"step": step_id, "required": step["required"], "status": payload["status"], "reason": payload["reason"]})
    write(run / "results/checks.json", checks)
    manifest = read(run / "manifest.json")
    manifest["compatibility"].setdefault("manual", {})[step_id] = {
        "procedure": payload["procedure"], "configuration": payload["configuration"],
        "limitations": payload["limitations"]}
    write(run / "manifest.json", manifest)
    refresh(run)


def compare(base, candidate, tolerance):
    verify_record(base)
    verify_record(candidate)
    a, b = read(base / "manifest.json"), read(candidate / "manifest.json")
    if a.get("status") != "complete" or b.get("status") != "complete" or not a.get("quality_accepted") or not b.get("quality_accepted"):
        raise ValueError("Comparison requires reviewed complete records")
    if a["compatibility"] != b["compatibility"] or a["build_environment"] != b["build_environment"]:
        raise ValueError("Incompatible scenario/environment/build configuration")
    old, new = read(base / "results/summary.json"), read(candidate / "results/summary.json")
    index = lambda rows: {(r["step"], r["name"], r["unit"], r["direction"]): r for r in rows}
    old, new = index(old), index(new)
    if old.keys() != new.keys():
        raise ValueError("Metric coverage differs")
    result = []
    for key, row in sorted(new.items()):
        before, after = old[key]["median"], row["median"]
        if old[key]["count"] != row["count"]:
            raise ValueError("Repetition counts differ")
        delta = (after - before) / abs(before) if before != 0 else None
        regression = False
        if row["direction"] != "descriptive":
            regression = (after > before if row["direction"] == "lower" else after < before) if delta is None else (
                delta > tolerance if row["direction"] == "lower" else -delta > tolerance)
        result.append({"step": key[0], "metric": key[1], "unit": key[2], "direction": key[3],
                       "base_median": before, "candidate_median": after,
                       "relative_change": delta, "regression": regression})
    return {"tolerance": tolerance, "results": result, "regression": any(r["regression"] for r in result)}


def archive(run, output):
    verify_record(run)
    output = Path(output).resolve()
    if output.is_relative_to(run.resolve()) or output.exists() or Path(str(output) + ".json").exists():
        raise ValueError("Archive must be new and outside the record")
    with tarfile.open(output, "w:gz") as tar:
        tar.add(run, arcname="record")
        for item in read(run / "manifest.json").get("reproduction_inputs", []):
            if digest(item["path"]) != item["sha256"]:
                raise ValueError("Source/resource snapshot changed or is unavailable")
            tar.add(item["path"], arcname="inputs/" + item["name"])
    # Verify a fresh extraction before advertising the archive as retrievable.
    with tempfile.TemporaryDirectory() as temporary:
        with tarfile.open(output) as tar:
            tar.extractall(temporary, filter="data")
        verify_record(Path(temporary) / "record")
        for item in read(run / "manifest.json").get("reproduction_inputs", []):
            if digest(Path(temporary) / "inputs" / item["name"]) != item["sha256"]:
                raise ValueError("Retrieved source/resource snapshot checksum mismatch")
    write(str(output) + ".json", {"archive": output.name, "sha256": digest(output),
                                 "retrieval_verified": True})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("doctor")
    p = sub.add_parser("preflight-web"); p.add_argument("--source", required=True)
    p = sub.add_parser("setup-fuzz"); p.add_argument("--work", required=True); p.add_argument("--cargo-fuzz", required=True)
    p = sub.add_parser("prepare"); p.add_argument("--work", required=True); p.add_argument("--revision", required=True)
    p.add_argument("--cpus", help="Allowed logical CPUs, e.g. 0-3,8")
    p.add_argument("--jobs", type=int)
    p.add_argument("--disk-floor-gib", type=float, default=0)
    p.add_argument("--incremental", action="store_true", help="Enable incremental builds (default off)")
    p = sub.add_parser("setup-unit"); p.add_argument("--work", required=True); p.add_argument("--unit", choices=("python", "web", "editor"), required=True); p.add_argument("--wheelhouse")
    p = sub.add_parser("run"); p.add_argument("--work", required=True); p.add_argument("--scenario", required=True); p.add_argument("--name", required=True); p.add_argument("--patch")
    p = sub.add_parser("export-patch"); p.add_argument("--source", required=True); p.add_argument("--base", required=True); p.add_argument("--output", required=True)
    p = sub.add_parser("replay-patch"); p.add_argument("--repo", required=True); p.add_argument("--base", required=True); p.add_argument("--patch", required=True)
    p = sub.add_parser("attach"); p.add_argument("record"); p.add_argument("--step", required=True); p.add_argument("--evidence", required=True)
    p = sub.add_parser("seal"); p.add_argument("record"); p.add_argument("--reviewer", required=True); p.add_argument("--preserve-incomplete", action="store_true"); p.add_argument("--reason")
    p = sub.add_parser("verify"); p.add_argument("record")
    p = sub.add_parser("archive"); p.add_argument("record"); p.add_argument("--output", required=True)
    p = sub.add_parser("compare"); p.add_argument("base"); p.add_argument("candidate"); p.add_argument("--tolerance", required=True, type=float)
    args = parser.parse_args()
    try:
        if args.action == "doctor":
            print(json.dumps(inspect_environment(), indent=2))
        elif args.action == "preflight-web":
            print(json.dumps(web_preflight(Path(args.source)), indent=2))
        elif args.action == "setup-fuzz":
            from fuzz_tools import attest
            print(attest(args.work, args.cargo_fuzz))
        elif args.action == "prepare":
            prepare(args.work, args.revision, {"cpus": args.cpus, "jobs": args.jobs, "disk_floor_gib": args.disk_floor_gib, "incremental": args.incremental})
        elif args.action == "setup-unit":
            setup_unit(args.work, args.unit, args.wheelhouse)
        elif args.action == "run":
            return run_campaign(args.work, args.scenario, args.name, args.patch)
        elif args.action == "export-patch":
            print(json.dumps(export_patch(args.source, args.base, args.output)))
        elif args.action == "replay-patch":
            print(json.dumps(replay(args.repo, args.base, args.patch)))
        elif args.action == "attach":
            attach(Path(args.record), args.step, args.evidence)
        elif args.action == "seal":
            seal_record(Path(args.record), args.reviewer, args.preserve_incomplete, args.reason)
        elif args.action == "verify":
            verify_record(Path(args.record)); print("Record verified")
        elif args.action == "archive":
            archive(Path(args.record), args.output)
        elif args.action == "compare":
            if not math.isfinite(args.tolerance) or args.tolerance < 0:
                raise ValueError("Tolerance must be finite and nonnegative")
            result = compare(Path(args.base), Path(args.candidate), args.tolerance)
            print(json.dumps(result, indent=2)); return int(result["regression"])
    except (ValueError, OSError, subprocess.SubprocessError, KeyError, StopIteration) as exc:
        print(f"Audit error: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
