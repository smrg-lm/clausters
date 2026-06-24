#!/usr/bin/env python3
"""Build the cargo artifacts and stage them inside the package for wheels.

The Clausters Python package is pure Python at runtime, but reaches the Rust
core through artifacts built by cargo, not pip:

- ``clausters-ffi`` -> ``libclausters_ffi`` (the numeric core: :mod:`clausters._native`)
- the ``clausters`` crate built with ``embed,realtime`` -> ``libclausters``
  (the embedded server / offline render: :mod:`clausters.ipc`)
- the ``clausters`` binary (default features) -> the **standalone server**
  shipped as the wheel's ``clausters`` command (a separate, networked or
  shared-memory server process; the embedded one above runs in-process).

This module builds them and copies the cdylibs into ``clausters/_libs/`` and the
binary into ``clausters/_bin/`` so they ship with the wheel (and are picked up
by an editable install). It is imported by ``setup.py`` and is also runnable on
its own to stage the artifacts ahead of a plain ``pip install``::

    python clients/python/build_native.py            # release (default)
    python clients/python/build_native.py --debug     # debug profile

Environment knobs (also honoured by ``setup.py``):

- ``CLAUSTERS_WORKSPACE``        path to the cargo workspace root (auto-detected
                                 by searching upward otherwise).
- ``CLAUSTERS_SKIP_NATIVE_BUILD`` if set, never run cargo; package whatever is
                                 already staged in ``clausters/_libs/`` and
                                 ``clausters/_bin/``.
- ``CLAUSTERS_CARGO_FEATURES``   features for the embed library
                                 (default ``embed,realtime``).
- ``CLAUSTERS_CARGO_PROFILE``    ``release`` (default) or ``debug``.
"""

import os
import platform
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PKG_DIR = os.path.join(HERE, "clausters")
LIBS_DIR = os.path.join(PKG_DIR, "_libs")
BIN_DIR = os.path.join(PKG_DIR, "_bin")

# The cdylib stems cargo builds and the crate that produces each.
_CRATES = {
    "clausters_ffi": ("clausters-ffi", None),  # no extra features
    "clausters": ("clausters", "embed,realtime"),  # overridable below
}


def bin_name() -> str:
    """Platform file name of the standalone server binary."""
    return "clausters.exe" if platform.system() == "Windows" else "clausters"


def _dylib_names(stem: str) -> list[str]:
    """Platform shared-library file name(s) for a cargo crate ``stem``."""
    system = platform.system()
    if system == "Darwin":
        return [f"lib{stem}.dylib"]
    if system == "Windows":
        return [f"{stem}.dll"]
    return [f"lib{stem}.so"]


def _is_workspace(path: str) -> bool:
    cargo = os.path.join(path, "Cargo.toml")
    if not os.path.isfile(cargo):
        return False
    try:
        with open(cargo, encoding="utf-8") as f:
            return "[workspace]" in f.read()
    except OSError:
        return False


def find_workspace() -> str | None:
    """The cargo workspace root: ``CLAUSTERS_WORKSPACE`` or the nearest ancestor
    with a ``[workspace]`` ``Cargo.toml``. ``None`` if not reachable (e.g. an
    isolated build that copied only this directory)."""
    env = os.environ.get("CLAUSTERS_WORKSPACE")
    if env:
        return os.path.abspath(env) if _is_workspace(env) else None
    path = HERE
    while True:
        if _is_workspace(path):
            return path
        parent = os.path.dirname(path)
        if parent == path:
            return None
        path = parent


def staged_libs() -> list[str]:
    """The cdylibs already present in ``clausters/_libs/`` for this platform."""
    if not os.path.isdir(LIBS_DIR):
        return []
    wanted = {n for stem in _CRATES for n in _dylib_names(stem)}
    return [os.path.join(LIBS_DIR, n) for n in wanted
            if os.path.exists(os.path.join(LIBS_DIR, n))]


def staged_bin() -> str | None:
    """The standalone binary staged in ``clausters/_bin/``, or ``None``."""
    path = os.path.join(BIN_DIR, bin_name())
    return path if os.path.exists(path) else None


def _cargo_build(workspace: str, crate: str, features: str | None, profile: str):
    cmd = ["cargo", "build", "-p", crate]
    if profile == "release":
        cmd.append("--release")
    if features:
        cmd += ["--features", features]
    print("clausters: " + " ".join(cmd))
    subprocess.run(cmd, cwd=workspace, check=True)


def _cargo_build_bin(workspace: str, profile: str):
    """Build the standalone server binary with default features."""
    cmd = ["cargo", "build", "--bin", "clausters"]
    if profile == "release":
        cmd.append("--release")
    print("clausters: " + " ".join(cmd))
    subprocess.run(cmd, cwd=workspace, check=True)


def stage(workspace: str, profile: str) -> list[str]:
    """Copy every freshly built cdylib into ``clausters/_libs/``."""
    target = os.path.join(workspace, "target", profile)
    os.makedirs(LIBS_DIR, exist_ok=True)
    copied = []
    for stem in _CRATES:
        for name in _dylib_names(stem):
            src = os.path.join(target, name)
            if os.path.exists(src):
                shutil.copy2(src, os.path.join(LIBS_DIR, name))
                copied.append(name)
    return copied


def stage_binary(workspace: str, profile: str) -> str | None:
    """Copy the freshly built standalone binary into ``clausters/_bin/``,
    preserving its executable bit (``copy2``). Returns its name or ``None``."""
    src = os.path.join(workspace, "target", profile, bin_name())
    if not os.path.exists(src):
        return None
    os.makedirs(BIN_DIR, exist_ok=True)
    shutil.copy2(src, os.path.join(BIN_DIR, bin_name()))
    return bin_name()


def build_and_stage(profile: str = "release", *, allow_skip: bool = False) -> list[str]:
    """Build the cdylibs (unless skipped) and stage them; return staged names.

    With ``allow_skip`` (used from ``setup.py``), a missing workspace or a
    ``CLAUSTERS_SKIP_NATIVE_BUILD`` request falls back to the libs already
    staged instead of failing, so an isolated build of a pre-staged tree still
    produces a valid wheel."""
    skip = bool(os.environ.get("CLAUSTERS_SKIP_NATIVE_BUILD"))
    workspace = find_workspace()

    if skip or workspace is None:
        already = [os.path.basename(p) for p in staged_libs()]
        if staged_bin():
            already.append(bin_name())
        reason = ("CLAUSTERS_SKIP_NATIVE_BUILD set" if skip
                  else "cargo workspace not found (set CLAUSTERS_WORKSPACE)")
        if already and (allow_skip or skip):
            print(f"clausters: {reason}; using staged artifacts: {', '.join(already)}")
            return already
        raise SystemExit(
            f"clausters: {reason} and nothing staged in clausters/_libs/. "
            "Run this from inside the repo (it finds the workspace), set "
            "CLAUSTERS_WORKSPACE, or pre-stage the artifacts."
        )

    features = os.environ.get("CLAUSTERS_CARGO_FEATURES")
    for stem, (crate, default_feat) in _CRATES.items():
        feat = features if (features and stem == "clausters") else default_feat
        _cargo_build(workspace, crate, feat, profile)
    copied = stage(workspace, profile)
    if not copied:
        raise SystemExit("clausters: cargo produced no cdylibs to stage")
    # The standalone server binary (default features), bundled so the wheel's
    # `clausters` command can run a separate (networked / shared-memory) server.
    _cargo_build_bin(workspace, profile)
    binname = stage_binary(workspace, profile)
    if binname:
        copied.append(binname)
    print("clausters: staged " + ", ".join(copied)
          + f" into {LIBS_DIR}" + (f" and {BIN_DIR}" if binname else ""))
    return copied


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    profile = os.environ.get("CLAUSTERS_CARGO_PROFILE", "release")
    if "--debug" in argv:
        profile = "debug"
    if "--release" in argv:
        profile = "release"
    build_and_stage(profile)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
