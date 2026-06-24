"""setuptools shim that bundles the cargo-built artifacts into the wheel.

Configuration lives in ``pyproject.toml``; this file exists only to hook the
native build. Before the package files are collected, it builds the cdylibs and
the standalone server binary with cargo and stages them in ``clausters/_libs/``
and ``clausters/_bin/`` (see ``build_native.py``), so the resulting wheel
carries both the embedded core and the standalone server, self-contained.

Both travel as package data: the cdylibs back the in-process embedded server,
and the standalone binary in ``clausters/_bin/`` is exposed by the ``clausters``
console-script (see ``clausters._cli``), which locates and execs it. (Shipping a
native binary through the wheel's ``scripts=`` slot fails: setuptools'
``build_scripts`` parses every script as Python source and chokes on the ELF.)

Because the wheel ships a compiled ``.so``/``.dylib``/``.dll`` (and a native
binary) it is *not* platform-independent: :class:`_PlatformWheel` marks it so the
wheel gets the right ``linux_x86_64`` / ``macosx_*`` / ``win_amd64`` tag instead
of the bogus ``py3-none-any``.
"""

import os
import sys

from setuptools import setup
from setuptools.command.build_py import build_py

# The PEP 517 backend may exec this file without its directory on sys.path.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_native

try:  # modern setuptools ships its own bdist_wheel; older defers to `wheel`.
    from setuptools.command.bdist_wheel import bdist_wheel as _bdist_wheel
except ImportError:  # pragma: no cover
    try:
        from wheel.bdist_wheel import bdist_wheel as _bdist_wheel
    except ImportError:  # only when building an sdist
        _bdist_wheel = None


class _BuildPy(build_py):
    """Stage the native cdylibs and the standalone binary before the normal
    package-file collection, so both ship as package data."""

    def run(self):
        # allow_skip: an isolated/copied build that cannot see the workspace
        # falls back to pre-staged artifacts instead of failing.
        build_native.build_and_stage(allow_skip=True)
        super().run()


cmdclass = {"build_py": _BuildPy}

if _bdist_wheel is not None:

    class _PlatformWheel(_bdist_wheel):
        def finalize_options(self):
            super().finalize_options()
            # Has a compiled extension in spirit: force a platform-tagged wheel.
            self.root_is_pure = False

        def get_tag(self):
            # The code is pure Python + ctypes (no CPython ABI linkage), so the
            # wheel is platform-specific but Python-version agnostic: py3-none-<plat>.
            _py, _abi, plat = super().get_tag()
            return "py3", "none", plat

    cmdclass["bdist_wheel"] = _PlatformWheel


setup(cmdclass=cmdclass)
