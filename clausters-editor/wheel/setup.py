"""setuptools shim that stages the Tauri build into the wheel.

Configuration lives in ``pyproject.toml``; this file builds the application
with ``npm run tauri build -- --no-bundle`` (the frontend and the driver are
compiled into the binary) and stages, inside ``clausters_editor/``, the binary
and the resources Tauri's ``resource_dir`` looks for beside it:

- Linux: ``bin/clausters-editor`` and ``lib/clausters-editor/{docs,licenses}``
  (``../lib/<name>`` from the binary);
- macOS: ``bin/clausters-editor`` and ``Resources/{docs,licenses}``;
- Windows: ``bin/clausters-editor.exe`` with ``docs`` and ``licenses`` beside it.

``CLAUSTERS_EDITOR_SKIP_BUILD=1`` stages the binary already built instead of
building it.

The wheel is tagged for its platform (``py3-none-<plat>``): it carries a native
binary and no CPython extension. On Linux the tag is ``manylinux_X_Y`` with the
build machine's glibc, which is the oldest the binary runs on -- so a wheel
meant for every distribution with WebKitGTK 4.1 is built on the oldest of them.
"""

import json
import os
import shutil
import subprocess
import sys

from setuptools import setup
from setuptools.command.build_py import build_py

HERE = os.path.dirname(os.path.abspath(__file__))
APP = os.path.dirname(HERE)  # clausters-editor/
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
    with open(os.path.join(APP, "src-tauri", "tauri.conf.json"), encoding="utf-8") as f:
        return json.load(f)["version"]


def _resource_dir():
    """Where Tauri's `resource_dir` resolves for a binary in ``bin/``."""
    if sys.platform.startswith("linux"):
        return os.path.join(PKG, "lib", NAME)
    if sys.platform == "darwin":
        return os.path.join(PKG, "Resources")
    return os.path.join(PKG, "bin")


def stage():
    """Build the application (unless skipped) and copy it into the package."""
    built = os.path.join(APP, "src-tauri", "target", "release", EXE)
    if not os.environ.get("CLAUSTERS_EDITOR_SKIP_BUILD"):
        npm = shutil.which("npm")
        if npm is None:
            raise SystemExit("clausters-editor: building the wheel needs npm and cargo")
        if not os.path.isdir(os.path.join(APP, "node_modules")):
            subprocess.check_call([npm, "ci"], cwd=APP)
        subprocess.check_call([npm, "run", "tauri", "build", "--", "--no-bundle"], cwd=APP)
    if not os.path.exists(built):
        raise SystemExit(f"clausters-editor: {built} does not exist; build it first")

    for stale in ("bin", "lib", "Resources"):
        shutil.rmtree(os.path.join(PKG, stale), ignore_errors=True)
    os.makedirs(os.path.join(PKG, "bin"))
    shutil.copy2(built, os.path.join(PKG, "bin", EXE))
    res = _resource_dir()
    shutil.copytree(os.path.join(APP, "docs"), os.path.join(res, "docs"))
    shutil.copytree(os.path.join(APP, "licenses"), os.path.join(res, "licenses"))


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
_copying = os.path.join(APP, "COPYING")
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
