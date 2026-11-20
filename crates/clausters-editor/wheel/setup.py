"""setuptools shim that stages the editor's build into the wheel.

Configuration lives in ``pyproject.toml``; this file builds the editor with
``cargo build -p clausters-editor --release`` from the repository's root (the
driver of its session is compiled into the binary) and stages, inside
``clausters_editor/``:

- ``bin/clausters-editor``, the binary;
- ``share/clausters-editor/{docs,licenses}``, its book (which the binary
  finds at ``../share/clausters-editor/docs``) and the third-party notices;
- ``_libs/libverovio.so`` and its SMuFL data (``_libs/verovio/``), the
  engraver the binary links (its run path names ``$ORIGIN/../_libs``), on
  Linux.

``CLAUSTERS_EDITOR_SKIP_BUILD=1`` stages the binary already built instead of
building it; ``CARGO_TARGET_DIR`` says where cargo builds.

The wheel is tagged for its platform (``py3-none-<plat>``): it carries a native
binary and no CPython extension. On Linux the tag is ``manylinux_X_Y`` with the
build machine's glibc, which is the oldest the binary runs on -- so a wheel
meant for every distribution is built on the oldest of them.
"""

import os
import re
import shutil
import subprocess
import sys

from setuptools import setup
from setuptools.command.build_py import build_py

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)  # crates/clausters-editor/
ROOT = os.path.dirname(os.path.dirname(CRATE))  # the repository
PKG = os.path.join(HERE, "clausters_editor")
NAME = "clausters-editor"
EXE = NAME + ".exe" if os.name == "nt" else NAME

try:  # modern setuptools ships its own bdist_wheel; older defers to `wheel`.
    from setuptools.command.bdist_wheel import bdist_wheel as _bdist_wheel
except ImportError:  # pragma: no cover
    try:
        from wheel.bdist_wheel import bdist_wheel as _bdist_wheel
    except ImportError:  # only when building an sdist
        _bdist_wheel = None


def _version():
    """The workspace's version, which the crate takes (`version.workspace`)."""
    with open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8") as f:
        text = f.read()
    table = text.split("[workspace.package]", 1)[1]
    found = re.search(r'^version\s*=\s*"([^"]+)"', table, re.MULTILINE)
    if found is None:
        raise SystemExit("clausters-editor: the workspace's Cargo.toml names no version")
    return found.group(1)


def _built():
    target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target")
    return os.path.join(target, "release", EXE)


def _strip(path):
    """Best-effort strip of debug symbols from a staged binary (POSIX), as
    the client's wheel strips its own. A missing ``strip`` tool or a Windows
    build leaves it as-is."""
    if os.name == "nt" or shutil.which("strip") is None:
        return
    try:
        subprocess.run(["strip", path], check=True)
    except (OSError, subprocess.CalledProcessError):
        pass


def _linked(binary, soname):
    """Where the system's loader finds `soname` for `binary`, or None."""
    try:
        out = subprocess.run(["ldd", binary], capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    for line in out.splitlines():
        name, _, rest = line.strip().partition(" => ")
        if name == soname and rest.startswith("/"):
            return rest.split(" (", 1)[0]
    return None


def _stage_verovio(binary):
    """libverovio and its SMuFL data into ``_libs/``, from where the built
    binary links it (or ``VEROVIO_PREFIX``). A binary built without the
    engraver links none, and nothing is staged."""
    if not sys.platform.startswith("linux"):
        return
    lib = _linked(binary, "libverovio.so")
    prefix = os.environ.get("VEROVIO_PREFIX")
    if lib is None and prefix:
        lib = os.path.join(prefix, "lib", "libverovio.so")
    if lib is None or not os.path.exists(lib):
        return
    libs = os.path.join(PKG, "_libs")
    os.makedirs(libs)
    shutil.copy2(lib, os.path.join(libs, "libverovio.so"))
    _strip(os.path.join(libs, "libverovio.so"))
    data = os.path.join(os.path.dirname(os.path.dirname(lib)), "share", "verovio")
    if os.path.isdir(data):
        shutil.copytree(data, os.path.join(libs, "verovio"))


def stage():
    """Build the editor (unless skipped) and copy it into the package."""
    built = _built()
    if not os.environ.get("CLAUSTERS_EDITOR_SKIP_BUILD"):
        cargo = shutil.which("cargo")
        if cargo is None:
            raise SystemExit("clausters-editor: building the wheel needs cargo")
        subprocess.check_call([cargo, "build", "-p", NAME, "--release"], cwd=ROOT)
    if not os.path.exists(built):
        raise SystemExit(f"clausters-editor: {built} does not exist; build it first")

    for stale in ("bin", "share", "_libs"):
        shutil.rmtree(os.path.join(PKG, stale), ignore_errors=True)
    os.makedirs(os.path.join(PKG, "bin"))
    shutil.copy2(built, os.path.join(PKG, "bin", EXE))
    _strip(os.path.join(PKG, "bin", EXE))
    share = os.path.join(PKG, "share", NAME)
    shutil.copytree(os.path.join(CRATE, "docs"), os.path.join(share, "docs"))
    shutil.copytree(os.path.join(CRATE, "licenses"), os.path.join(share, "licenses"))
    _stage_verovio(built)


class _BuildPy(build_py):
    """Stage the application before the package files are collected."""

    def run(self):
        stage()
        # `build_lib` outlives a build and copying only adds to it: a file
        # removed from the app (a page of its docs) would stay there and go
        # into the next wheel. The package is staged fresh above, so its copy
        # is started fresh too.
        shutil.rmtree(os.path.join(self.build_lib, "clausters_editor"), ignore_errors=True)
        super().run()


def _manylinux(plat):
    """Map a bare ``linux_<arch>`` tag to ``manylinux_<glibc>_<arch>``: PyPI
    rejects plain ``linux_*``, and the binary runs on the build machine's glibc
    and later. Non-glibc or non-Linux platforms keep their tag."""
    if not plat.startswith("linux_"):
        return plat
    try:
        libc = os.confstr("CS_GNU_LIBC_VERSION")  # e.g. "glibc 2.35"
    except (AttributeError, OSError, ValueError):
        libc = None
    if not libc or not libc.startswith("glibc "):
        return plat
    major, _, minor = libc.split()[1].partition(".")
    return f"manylinux_{major}_{minor}_{plat[len('linux_'):]}"


# The repository's license, where `license-files` reads it.
_copying = os.path.join(ROOT, "COPYING")
if os.path.exists(_copying):
    shutil.copy2(_copying, os.path.join(HERE, "COPYING"))

cmdclass = {"build_py": _BuildPy}

if _bdist_wheel is not None:

    class _PlatformWheel(_bdist_wheel):
        def finalize_options(self):
            super().finalize_options()
            self.root_is_pure = False

        def get_tag(self):
            _py, _abi, plat = super().get_tag()
            return "py3", "none", _manylinux(plat)

    cmdclass["bdist_wheel"] = _PlatformWheel


setup(version=_version(), cmdclass=cmdclass)
