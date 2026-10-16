"""Campaign resource policy and prerequisite identities (standard library only)."""
import hashlib
import math
import os
from pathlib import Path
import shutil


def validate_policy(value):
    value = dict(value)
    cpus = value.get("cpus")
    if isinstance(cpus, str):
        selected = set()
        try:
            for part in cpus.split(","):
                limits = list(map(int, part.split("-")))
                if len(limits) not in (1, 2) or limits[0] < 0 or limits[-1] < limits[0]:
                    raise ValueError()
                selected.update(range(limits[0], limits[-1] + 1))
        except ValueError:
            raise ValueError("Invalid CPU list") from None
        cpus = sorted(selected)
    if cpus is not None:
        if not cpus or any(type(c) is not int or c < 0 for c in cpus):
            raise ValueError("Invalid CPU list")
        if not hasattr(os, "sched_getaffinity") or not set(cpus) <= os.sched_getaffinity(0):
            raise ValueError("Requested CPUs are unavailable in the current affinity")
    jobs = value.get("jobs")
    if jobs is None and cpus:
        jobs = len(cpus)
    if jobs is not None and (type(jobs) is not int or jobs <= 0):
        raise ValueError("Jobs must be a positive integer")
    floor = value.get("disk_floor_gib", 0)
    if isinstance(floor, bool) or not isinstance(floor, (int, float)) or not math.isfinite(floor) or floor < 0:
        raise ValueError("Disk floor must be finite and nonnegative")
    if type(value.get("incremental", False)) is not bool:
        raise ValueError("Incremental must be boolean")
    return {"cpus": cpus, "jobs": jobs, "disk_floor_gib": floor,
            "incremental": value.get("incremental", False)}


def policy_environment(policy, work):
    policy = validate_policy(policy)
    if shutil.disk_usage(work).free < policy["disk_floor_gib"] * 1024 ** 3:
        raise ValueError("Free disk space is below campaign floor")
    env = {"AUDIT_DISK_FLOOR_GIB": str(policy["disk_floor_gib"]), "CARGO_INCREMENTAL": "1" if policy["incremental"] else "0"}
    if policy["jobs"]:
        env.update(CARGO_BUILD_JOBS=str(policy["jobs"]), RUST_TEST_THREADS=str(policy["jobs"]))
    if policy["cpus"]:
        env["AUDIT_CPUS"] = ",".join(map(str, policy["cpus"]))
    return env


def target_for(base, workspace):
    workspace = str(workspace)
    if workspace.startswith("clients/gui"):
        return Path(str(base) + "-gui")
    if workspace.startswith("clausters-editor"):
        return Path(str(base) + "-editor")
    if workspace.startswith("audit/fuzz"):
        return Path(str(base) + "-fuzz")
    return Path(base)


def web_preflight(source):
    names = ["verovio/verovio.js", "verovio/verovio.wasm", "faust/libfaust-wasm.js",
             "faust/libfaust-wasm.wasm", "faust/libfaust-wasm.data", "faust/version"]
    root = Path(source) / "clients/web/vendor"
    missing = [n for n in names if not (root / n).is_file() or not (root / n).stat().st_size]
    if missing:
        raise ValueError("Missing web vendor assets: " + ", ".join(missing))
    return {"assets": {n: hashlib.sha256((root / n).read_bytes()).hexdigest() for n in names},
            "pins": {n: hashlib.sha256((Path(source) / n).read_bytes()).hexdigest()
                     for n in ("third_party/faust.pin", "third_party/verovio.pin")},
            "provenance": "Content identity only; retain build/source receipts separately"}


def receipt_environment(env):
    keys = ("PATH", "PYTHONPATH", "CARGO_TARGET_DIR", "CARGO_INCREMENTAL", "CARGO_BUILD_JOBS",
            "CARGO_NET_OFFLINE", "RUST_TEST_THREADS", "AUDIT_CPUS", "AUDIT_DISK_FLOOR_GIB", "AUDIT_SOURCE", "AUDIT_TARGET",
            "AUDIT_REAL_CARGO", "RUSTFLAGS", "RUSTDOCFLAGS", "RUSTUP_TOOLCHAIN", "CC", "CXX",
            "FAUST_PREFIX", "VEROVIO_PREFIX", "BINDGEN_EXTRA_CLANG_ARGS", "LIBCLANG_PATH", "PIPEWIRE_QUANTUM", "CLAUSTERS_BIN", "CLAUSTERS_GUI_BIN")
    return {k: env[k] for k in keys if k in env}


def validate_build_contract(step, source):
    import tomllib
    argv = step["argv"]
    if not argv or argv[0] != "cargo":
        return
    profile = "release" if "--release" in argv else "debug"
    if "--profile" in argv:
        profile = argv[argv.index("--profile") + 1]
        if profile == "dev":
            profile = "debug"
    if profile != step.get("profile", profile):
        raise ValueError("Declared profile differs from Cargo arguments")
    features = []
    if "--features" in argv:
        features = re_split(argv[argv.index("--features") + 1])
    if step.get("features") is not None and sorted(features) != sorted(step["features"]):
        raise ValueError("Declared features differ from Cargo arguments")
    manifest = Path(source) / step.get("cwd", ".") / "Cargo.toml"
    if "--manifest-path" in argv:
        manifest = manifest.parent / argv[argv.index("--manifest-path") + 1]
    if features:
        available = tomllib.loads(manifest.read_text()).get("features", {})
        if any(f not in available and "/" not in f for f in features):
            raise ValueError("Unknown feature for selected workspace")


def re_split(value):
    import re
    return [f for f in re.split(r"[, ]+", value) if f]
