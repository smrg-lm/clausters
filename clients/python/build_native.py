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
  It is also what makes the wheel heavy (~55 MB packed): the Faust compiler is
  LLVM.
- ``verovio``, the engraver behind the `score` widget, unpacked whole into
  ``_libs/verovio/`` -> notation engraves and edits from an installed wheel with
  nothing else present. Same bundling reason as libLLVM, plus a stronger one:
  the *published* verovio cannot edit at all (see ``third_party/verovio.pin``),
  so this copy is preferred over any installed one. ~4.8 MB packed.

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

import glob
import os
import platform
import shutil
import subprocess
import sys
import zipfile

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


# Baseline shared libraries every glibc target is assumed to provide. These are
# never vendored: bundling the C/C++ runtime or the loader would pin a copy older
# than (or ABI-incompatible with) the host's and break everything that links them
# system-wide. Matches the spirit of auditwheel's policy whitelist, scoped to what
# libfaust/libLLVM can pull in. Everything else in their transitive closure *is*
# vendored (see ``stage_faust_libs``).
_SYSTEM_SONAME_PREFIXES = (
    "libc.so", "libm.so", "libdl.so", "librt.so", "libpthread.so",
    "libutil.so", "libresolv.so", "libnsl.so", "libgcc_s.so", "libstdc++.so",
    "ld-linux",
)


def stage_faust_libs(profile: str) -> list[str]:
    """Copy libfaust — and the libLLVM it needs, and *their* transitive deps —
    beside the cdylibs in ``_libs/``.

    The `faust` feature is on by default, so the built artifacts link libfaust
    dynamically, and libfaust in turn links the LLVM shared library that *is* its
    JIT. Bundling both is what makes an installed wheel able to compile a
    FaustDef on a machine with neither installed — the same self-contained
    packaging the ``clausters-gui`` binary gets. ``build.rs`` writes a `DT_RPATH`
    of ``$ORIGIN``/``$ORIGIN/../_libs``, inherited by transitive dependencies, so
    the loader finds these copies before (or without) any system ones.

    libLLVM does not stop at itself: it links libxml2, libzstd, libedit, libz,
    libffi, libtinfo… none of which are ours and none of which are guaranteed on
    the target. Worse, their sonames drift between distro generations — a wheel
    built where LLVM linked ``libxml2.so.2`` fails to load on a host that only
    ships ``libxml2.so.16`` (exactly the "cannot open shared object file" the
    standalone server dies with). So we vendor the **whole transitive closure**
    of libfaust/libLLVM, minus the baseline system libraries in
    ``_SYSTEM_SONAME_PREFIXES``. ``ldd`` already flattens the tree, so one pass
    over each root's resolved path captures deps-of-deps (libbsd -> libmd, …).

    The libraries are read off the *staged* artifacts, keyed by the exact soname
    the loader asks for. A server built without the feature needs no libfaust, so
    nothing is found and nothing is staged.
    """
    artifacts = [p for p in (staged_bin(), *staged_libs()) if p]
    # The two libraries we deliberately vendor (the Faust JIT compiler), each
    # keyed by the exact soname the loader asks for and its resolved build-host
    # path. We scope the closure to *these* roots, not to the whole binary, so we
    # never drag in the GUI's graphics/audio system stack.
    roots: dict[str, str] = {}
    for art in artifacts:
        for soname, resolved in _needed_libs(art).items():
            if soname.startswith(("libfaust.", "libLLVM.")):
                roots.setdefault(soname, resolved)
    wanted: dict[str, str] = dict(roots)
    for resolved in list(roots.values()):
        for soname, dep in _needed_libs(resolved).items():
            if soname.startswith(_SYSTEM_SONAME_PREFIXES):
                continue
            wanted.setdefault(soname, dep)
    if wanted and platform.system() == "Linux" and shutil.which("patchelf") is None:
        raise SystemExit(
            "clausters: patchelf is required to bundle libfaust/libLLVM into a "
            "relocatable wheel (it rewrites their run path to $ORIGIN). Install it "
            "with `pip install patchelf` (a self-contained wheel) or your package "
            "manager, then rebuild."
        )
    copied = []
    for soname, resolved in sorted(wanted.items()):
        dst = os.path.join(LIBS_DIR, soname)
        src = os.path.realpath(resolved)
        # Re-running staging resolves a root to its already-staged copy (the
        # artifacts' rpath includes ``_libs``); don't copy a file onto itself.
        if os.path.realpath(dst) != src:
            shutil.copy2(src, dst)
            _strip(dst)
        if platform.system() == "Linux":
            _set_origin_rpath(dst)
        copied.append(soname)
    return copied


def _verovio_wheel() -> str | None:
    """The verovio wheel our own recipe built, if it is there."""
    workspace = find_workspace()
    if workspace is None:
        return None
    dist = os.path.join(workspace, "third_party", "verovio", "dist-clausters")
    wheels = sorted(glob.glob(os.path.join(dist, "verovio-*.whl")),
                    key=os.path.getmtime, reverse=True)
    return wheels[0] if wheels else None


def stage_verovio() -> list[str]:
    """Unpack the engraver into ``_libs/verovio/`` so notation ships with the
    wheel.

    Same reasoning as libLLVM next to it: a third-party artifact we do not build
    into our own binaries, bundled so an installed wheel needs nothing else on
    the machine. Here it matters more than convenience, because the *published*
    verovio cannot do the job — 6.2.1's score editor is unreachable, so a client
    resolving `import verovio` from PyPI would engrave pages and then silently
    refuse every edit (`third_party/verovio.pin` has the diagnosis). Bundling our
    pinned build is what makes the editing round trip work at all, and
    `clausters.gui.notation` prefers this copy over anything installed.

    It is upstream's own Python package (an extension module plus its SMuFL
    resource data), so it is staged whole rather than file by file, and its
    ``__init__`` locates the data relative to wherever the package is found —
    which is what lets it live here instead of in site-packages. Its only shared
    dependencies are the C++/C runtime, so unlike libfaust there is no transitive
    closure to vendor and no run path to rewrite.
    """
    wheel = _verovio_wheel()
    if wheel is None:
        print("clausters: no verovio wheel in third_party/verovio/dist-clausters/; "
              "skipping (build it with third_party/build-verovio.sh --python)")
        return []
    dst = os.path.join(LIBS_DIR, "verovio")
    shutil.rmtree(dst, ignore_errors=True)
    with zipfile.ZipFile(wheel) as zf:
        members = [n for n in zf.namelist() if n.startswith("verovio/")
                   and "__pycache__/" not in n]
        zf.extractall(LIBS_DIR, members)
    for name in os.listdir(dst):
        if name.endswith(".so"):
            _strip(os.path.join(dst, name))
    return [f"verovio/ (from {os.path.basename(wheel)})"]


def _strip(path: str):
    """Best-effort strip of debug symbols from a staged binary (POSIX). A missing
    ``strip`` tool or a Windows build leaves it as-is."""
    if os.name == "nt" or shutil.which("strip") is None:
        return
    try:
        subprocess.run(["strip", path], check=True)
    except (OSError, subprocess.CalledProcessError):
        pass


def _set_origin_rpath(path: str):
    """Rewrite a vendored library's run path to ``$ORIGIN`` so it finds its
    siblings in ``_libs/`` (Linux only).

    libfaust/libLLVM and their transitive deps come from the build host, not our
    build, so they carry the host's run paths — libLLVM's is ``$ORIGIN/../lib``,
    a directory that does not exist in the wheel. Worse, libLLVM uses ``DT_RUNPATH``,
    which (unlike the ``DT_RPATH`` ``build.rs`` gives *our* artifacts) is **not**
    inherited down the dependency chain: the standalone binary's ``$ORIGIN/../_libs``
    is therefore not consulted for libLLVM's own deps (libxml2, libzstd, …), and
    the loader falls through to the system, whose soname may differ (the
    ``libxml2.so.2`` vs ``libxml2.so.16`` failure). Pointing every vendored lib at
    ``$ORIGIN`` — the same directory they all live in — makes each one resolve its
    direct deps locally, which covers the whole graph. This is what auditwheel
    does; here it must run in ``build_native`` because the release builds a plain
    wheel with no auditwheel/repair step."""
    subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", path], check=True)


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
    # verovio, the engraver behind the `score` widget: bundled for the same
    # reason as libLLVM above, and because the published one cannot edit.
    copied += stage_verovio()
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
