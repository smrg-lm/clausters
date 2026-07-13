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
- the ``clausters-gui`` binary (from the **independent** ``clients/gui`` cargo
  workspace) -> the **visual server** the launcher runs (`clausters.launch` /
  `clausters.Session.gui`). Bundled here too, stripped, so the one package is
  self-contained — server *and* GUI, no separate install.
- ``libfaust`` and the ``libLLVM`` it JIT-compiles with, copied out of the build
  machine's prefix (they are not ours to build) -> a `FaustDef` compiles on a
  machine with neither installed. The `faust` feature is on by default, so this
  is what keeps the two def families *equally* usable from an installed wheel.
  It is also what makes the wheel heavy (~50 MB packed): the Faust compiler is
  LLVM.

This module builds them and copies the cdylibs into ``clausters/_libs/`` and the
binaries into ``clausters/_bin/`` so they ship with the wheel (and are picked up
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
- ``CLAUSTERS_SKIP_GUI_BUILD``   if set, do not build/stage the heavy
                                 ``clausters-gui`` binary (a light, server-only
                                 wheel); a source checkout's ``clients/gui/target``
                                 binary is still used at runtime if present.
- ``CLAUSTERS_GUI_FEATURES``     extra cargo features for the GUI binary (e.g.
                                 ``standalone``); default none.
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


def gui_bin_name() -> str:
    """Platform file name of the ``clausters-gui`` visual-server binary."""
    return "clausters-gui.exe" if platform.system() == "Windows" else "clausters-gui"


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


def staged_gui_bin() -> str | None:
    """The GUI binary staged in ``clausters/_bin/``, or ``None``."""
    path = os.path.join(BIN_DIR, gui_bin_name())
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


def _gui_workspace(workspace: str) -> str:
    """The independent ``clausters-gui`` crate directory under the repo root."""
    return os.path.join(workspace, "clients", "gui")


def _cargo_build_gui(workspace: str, profile: str):
    """Build the ``clausters-gui`` binary in its own workspace (``clients/gui``)."""
    cmd = ["cargo", "build", "--bin", "clausters-gui"]
    if profile == "release":
        cmd.append("--release")
    features = os.environ.get("CLAUSTERS_GUI_FEATURES")
    if features:
        cmd += ["--features", features]
    print("clausters: " + " ".join(cmd) + " (in clients/gui)")
    subprocess.run(cmd, cwd=_gui_workspace(workspace), check=True)


def stage_gui_binary(workspace: str, profile: str) -> str | None:
    """Copy the freshly built ``clausters-gui`` binary into ``clausters/_bin/``
    and strip it. Returns its name or ``None``.

    The binary is heavy chiefly because of debug symbols; a release build is a
    fraction of a debug one, and stripping the staged copy trims it further
    (system libraries are dynamic, not embedded), so the wheel stays small
    without touching the developer's cargo profiles."""
    src = os.path.join(_gui_workspace(workspace), "target", profile, gui_bin_name())
    if not os.path.exists(src):
        return None
    os.makedirs(BIN_DIR, exist_ok=True)
    dst = os.path.join(BIN_DIR, gui_bin_name())
    shutil.copy2(src, dst)
    _strip(dst)
    return gui_bin_name()


def _needed_libs(path: str) -> dict[str, str]:
    """The shared libraries ``path`` links against, as ``{soname: resolved path}``.

    Linux only (``ldd``); other platforms return nothing, which simply means no
    Faust libraries are staged there.
    """
    if platform.system() != "Linux" or shutil.which("ldd") is None:
        return {}
    try:
        out = subprocess.run(["ldd", path], check=True, capture_output=True,
                             text=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return {}
    needed = {}
    for line in out.splitlines():
        # "  libfaust.so.2 => /home/u/.local/lib/libfaust.so.2 (0x00007f...)"
        if "=>" not in line:
            continue
        soname, _, rest = line.strip().partition("=>")
        resolved = rest.strip().split(" (")[0].strip()
        if soname.strip() and os.path.isfile(resolved):
            needed[soname.strip()] = resolved
    return needed


def stage_faust_libs(profile: str) -> list[str]:
    """Copy libfaust — and the libLLVM it needs — beside the cdylibs in ``_libs/``.

    The `faust` feature is on by default, so the built artifacts link libfaust
    dynamically, and libfaust in turn links the LLVM shared library that *is* its
    JIT. Bundling both is what makes an installed wheel able to compile a
    FaustDef on a machine with neither installed — the same self-contained
    packaging the ``clausters-gui`` binary gets. ``build.rs`` writes a `DT_RPATH`
    of ``$ORIGIN``/``$ORIGIN/../_libs``, inherited by transitive dependencies, so
    the loader finds these copies before (or without) any system ones.

    The libraries are read off the *staged* artifacts, keyed by the exact soname
    the loader asks for. A server built without the feature needs no libfaust, so
    nothing is found and nothing is staged.
    """
    artifacts = [p for p in (staged_bin(), *staged_libs()) if p]
    wanted: dict[str, str] = {}
    for art in artifacts:
        for soname, resolved in _needed_libs(art).items():
            if soname.startswith(("libfaust.", "libLLVM.")):
                wanted.setdefault(soname, resolved)
    copied = []
    for soname, resolved in sorted(wanted.items()):
        dst = os.path.join(LIBS_DIR, soname)
        shutil.copy2(os.path.realpath(resolved), dst)
        _strip(dst)
        copied.append(soname)
    return copied


def _strip(path: str):
    """Best-effort strip of debug symbols from a staged binary (POSIX). A missing
    ``strip`` tool or a Windows build leaves it as-is."""
    if os.name == "nt" or shutil.which("strip") is None:
        return
    try:
        subprocess.run(["strip", path], check=True)
    except (OSError, subprocess.CalledProcessError):
        pass


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
        if staged_gui_bin():
            already.append(gui_bin_name())
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
    # The visual server (clausters-gui), from its own workspace, bundled so the
    # one package is self-contained. Skippable for a light, server-only wheel.
    if not os.environ.get("CLAUSTERS_SKIP_GUI_BUILD"):
        _cargo_build_gui(workspace, profile)
        guiname = stage_gui_binary(workspace, profile)
        if guiname:
            copied.append(guiname)
    elif staged_gui_bin():
        copied.append(gui_bin_name())
    # libfaust + its libLLVM, read off the staged artifacts: the `faust` feature
    # is on by default, and bundling them is what lets an installed wheel
    # JIT-compile a FaustDef with nothing else on the machine.
    copied += stage_faust_libs(profile)
    print("clausters: staged " + ", ".join(copied)
          + f" into {LIBS_DIR} and {BIN_DIR}")
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
